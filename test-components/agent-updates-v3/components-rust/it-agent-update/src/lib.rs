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

    async fn load_snapshot(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() >= 8 {
            self.last = u64::from_be_bytes(bytes[..8].try_into().unwrap());
            Ok(())
        } else {
            Err("Invalid snapshot - not enough bytes to read u64".to_string())
        }
    }
}
