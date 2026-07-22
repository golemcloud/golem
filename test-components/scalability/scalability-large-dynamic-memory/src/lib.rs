use golem_rust::{agent_definition, agent_implementation};
use std::time::Duration;

const PAGE_SIZE: usize = 1_048_576; // 1 MB
const COUNT: usize = 512;

#[agent_definition]
pub trait LargeDynamicMemoryAgent {
    fn new(name: String) -> Self;
    fn run(&self) -> u64;
    fn run_with_delay(&self, delay_millis: u64) -> u64;
}

struct LargeDynamicMemoryAgentImpl {
    _name: String,
}

#[agent_implementation]
impl LargeDynamicMemoryAgent for LargeDynamicMemoryAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn run(&self) -> u64 {
        self.allocate()
    }

    fn run_with_delay(&self, delay_millis: u64) -> u64 {
        let result = self.allocate();
        std::thread::sleep(Duration::from_millis(delay_millis));
        result
    }
}

impl LargeDynamicMemoryAgentImpl {
    fn allocate(&self) -> u64 {
        let mut pages = Vec::with_capacity(COUNT);
        for i in 0..COUNT {
            let data = vec![0u8; PAGE_SIZE];
            println!("page {} first: {}", i, data[0]);
            println!("page {} last:  {}", i, data[PAGE_SIZE - 1]);
            pages.push(data);

            std::thread::sleep(Duration::from_micros(5));
        }

        drop(pages);
        0
    }
}
