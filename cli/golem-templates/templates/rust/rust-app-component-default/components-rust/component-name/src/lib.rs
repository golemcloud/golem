use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait CounterAgent {
    // The agent constructor, it's parameters identify the agent
    fn new(name: String) -> Self;

    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    _name: String,
    count: u32,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            count: 0,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}
