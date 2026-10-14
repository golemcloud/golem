use golem_rust::{agent_definition, agent_implementation};
use std::time::{Duration, Instant};

const PAGE_SIZE: usize = 1_048_576; // 1 MB
const COUNT: usize = 512;

#[agent_definition]
pub trait LargeDynamicMemoryAgent {
    fn new(name: String) -> Self;
    fn run(&self) -> u64;
    fn run_with_delay(&self, delay_millis: u64) -> u64;
    fn run_with_memory_and_work(&self, memory_mib: u64, work_millis: u64) -> u64;
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

    fn run_with_memory_and_work(&self, memory_mib: u64, work_millis: u64) -> u64 {
        let result = self.allocate_pages(memory_mib.try_into().unwrap());
        let started = Instant::now();
        let mut work = 0u64;
        while started.elapsed() < Duration::from_millis(work_millis) {
            // Keep durable clock reads coarse so this workload does not flood the oplog.
            for _ in 0..100_000_000 {
                work = std::hint::black_box(work.wrapping_add(1));
            }
        }
        result
    }
}

impl LargeDynamicMemoryAgentImpl {
    fn allocate(&self) -> u64 {
        self.allocate_pages(COUNT)
    }

    fn allocate_pages(&self, count: usize) -> u64 {
        let mut pages = Vec::with_capacity(count);
        for i in 0..count {
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
