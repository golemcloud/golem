use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait Counter {
    fn new(init: i32) -> Self;
    fn increment(&mut self) -> i32;
}

struct CounterImpl {
    init: i32,
}

#[agent_implementation]
impl Counter for CounterImpl {
    fn new(init: i32) -> Self {
        CounterImpl { init }
    }
    fn increment(&mut self) -> i32 {
        self.init += 1;
        self.init
    }
}
