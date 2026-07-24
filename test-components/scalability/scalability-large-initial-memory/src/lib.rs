use golem_rust::{agent_definition, agent_implementation};
use std::time::Duration;

// 512 MB allocation
const DATA_SIZE: usize = 536_870_912;

#[agent_definition]
pub trait LargeInitialMemoryAgent {
    fn new(name: String) -> Self;
    fn run(&self) -> u64;
}

struct LargeInitialMemoryAgentImpl {
    _name: String,
    data: Vec<u8>,
}

#[agent_implementation]
impl LargeInitialMemoryAgent for LargeInitialMemoryAgentImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            data: vec![0u8; DATA_SIZE],
        }
    }

    fn run(&self) -> u64 {
        println!("DATA:  {}", self.data.len());
        println!("first: {}", self.data[0]);
        println!("last:  {}", self.data[DATA_SIZE - 1]);

        std::thread::sleep(Duration::from_secs(2));

        self.data.len() as u64
    }
}
