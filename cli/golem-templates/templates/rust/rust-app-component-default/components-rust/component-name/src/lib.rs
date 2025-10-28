use golem_rust::{Schema, agent_definition, agent_implementation};

#[agent_definition]
trait CounterAgent {
    fn new(init: CounterId) -> Self;
    fn increment(&mut self) -> i32;
}

#[derive(Schema)]
struct CounterId {
    id: String,
}

struct CounterAgentImpl {
    init: i32,
    _id: CounterId,
}

#[agent_implementation]
impl CounterAgent for CounterAgentImpl {
    fn new(id: CounterId) -> Self {
        CounterAgentImpl { _id: id, init: 0 }
    }
    fn increment(&mut self) -> i32 {
        self.init += 1;
        self.init
    }
}

