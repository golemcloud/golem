use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait Empty {
    fn new() -> Self;
}

struct EmptyImpl;

#[agent_implementation]
impl Empty for EmptyImpl {
    fn new() -> Self {
        Self
    }
}
