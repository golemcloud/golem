use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait CounterAgent {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    count: u32,
    _name: String,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        CounterImpl {_name: name, count: 0 }
    }
    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}
