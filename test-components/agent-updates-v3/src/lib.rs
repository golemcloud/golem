use golem_rust::bindings::wasi::keyvalue::eventual::{Bucket, get};
use golem_rust::{agent_definition, agent_implementation, description};

#[agent_definition]
#[description("Update test agent V3")]
pub trait UpdateTest {
    fn new() -> Self;
    fn get(&self) -> u64;
    fn set(&mut self, value: u64) -> u64;
}

struct UpdateTestImpl {
    last: u64,
}

#[agent_implementation]
impl UpdateTest for UpdateTestImpl {
    fn new() -> Self {
        Self { last: 0 }
    }

    fn get(&self) -> u64 {
        self.last
    }

    fn set(&mut self, value: u64) -> u64 {
        self.last = value;
        self.last
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        let mut result = Vec::new();
        result.extend_from_slice(&self.last.to_be_bytes());
        Ok(result)
    }

    // The agent environment variable `LOAD_SNAPSHOT` steers the load: `exit` exits the guest,
    // and `read-key` reads a key whose service can hold the load.
    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        if bytes.len() < 8 {
            return Err("Invalid snapshot - not enough bytes to read u64".to_string());
        }
        let last = u64::from_be_bytes(bytes[..8].try_into().unwrap());
        match std::env::var("LOAD_SNAPSHOT").as_deref() {
            Ok("exit") => std::process::exit(1),
            Ok("read-key") => {
                let bucket = Bucket::open_bucket("snapshot-load").unwrap();
                let _ = get(&bucket, &last.to_string()).unwrap();
            }
            _ => {}
        }
        Ok(Self { last })
    }
}

/// Takes snapshots with the executor's default policy. Its load hook reads the key of the loaded
/// value from the bucket `snapshot-load`, a remote read.
#[agent_definition(snapshotting = "enabled")]
pub trait KeyValueLoadTest {
    fn new() -> Self;
    fn set(&mut self, value: u64) -> u64;
}

struct KeyValueLoadTestImpl {
    last: u64,
}

#[agent_implementation]
impl KeyValueLoadTest for KeyValueLoadTestImpl {
    fn new() -> Self {
        Self { last: 0 }
    }

    fn set(&mut self, value: u64) -> u64 {
        self.last = value;
        self.last
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.last.to_be_bytes().to_vec())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let last = u64::from_be_bytes(
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| "Invalid snapshot".to_string())?,
        );
        let bucket = Bucket::open_bucket("snapshot-load").map_err(|error| format!("{error:?}"))?;
        get(&bucket, &last.to_string()).map_err(|error| format!("{error:?}"))?;
        Ok(Self { last })
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

#[agent_definition(ephemeral)]
pub trait SnapshotUpdateTest {
    fn new() -> Self;
    fn loaded_snapshot_revision(&self) -> u32;
    fn replay_revision(&self) -> u32;
    fn write_file(&mut self, path: String, content: String);
    fn read_file(&self, path: String) -> Option<String>;
}

struct SnapshotUpdateTestImpl;

#[agent_implementation]
impl SnapshotUpdateTest for SnapshotUpdateTestImpl {
    fn new() -> Self {
        Self
    }

    fn loaded_snapshot_revision(&self) -> u32 {
        3
    }

    fn replay_revision(&self) -> u32 {
        3
    }

    fn write_file(&mut self, path: String, content: String) {
        std::fs::write(&path, content).expect("the agent writes the file");
    }

    fn read_file(&self, path: String) -> Option<String> {
        std::fs::read_to_string(&path).ok()
    }
}
