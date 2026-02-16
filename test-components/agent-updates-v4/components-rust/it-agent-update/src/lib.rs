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
}
