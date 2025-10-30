use golem_rust::{Schema, agent_definition, agent_implementation};

#[agent_definition]
trait Counter {
    fn new(init: CounterId) -> Self;
    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    count: u32,
    _id: CounterId,
}

#[agent_implementation]
impl Counter for CounterImpl {
    fn new(id: CounterId) -> Self {
        CounterImpl { _id: id, count: 0 }
    }
    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}

#[derive(Schema)]
struct CounterId {
    id: String,
}
