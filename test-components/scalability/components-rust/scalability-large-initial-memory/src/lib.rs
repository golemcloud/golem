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
}

#[agent_implementation]
impl LargeInitialMemoryAgent for LargeInitialMemoryAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn run(&self) -> u64 {
        // Allocate 512MB, matching the C version's static BSS allocation
        let data = vec![0u8; DATA_SIZE];
        println!("DATA:  {}", data.len());
        println!("first: {}", data[0]);
        println!("last:  {}", data[DATA_SIZE - 1]);

        std::thread::sleep(Duration::from_secs(2));

        data.len() as u64
    }
}
