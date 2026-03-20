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

    async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
        Err("Invalid snapshot - simulating failure".to_string())
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
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

    async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}
