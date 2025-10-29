use golem_rust::{Schema, agent_definition, agent_implementation};

#[agent_definition]
trait Counter {
    fn new(init: CounterId) -> Self;
    fn increment(&mut self) -> i32;
}

struct CounterImpl {
    init: i32,
    _id: CounterId,
}

#[agent_implementation]
impl Counter for CounterImpl {
    fn new(id: CounterId) -> Self {
        CounterImpl { _id: id, init: 0 }
    }
    fn increment(&mut self) -> i32 {
        self.init += 1;
        self.init
    }
}

#[derive(Schema)]
struct CounterId {
    id: String,
}
