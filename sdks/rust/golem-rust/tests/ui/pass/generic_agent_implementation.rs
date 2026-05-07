use golem_rust::agentic::Schema;
use golem_rust::{agent_definition, agent_implementation};
use std::fmt::Debug;

#[agent_definition]
trait GenericAgent<T: Schema + Clone + Debug> {
    fn new(id: String) -> Self;
    fn len(&self) -> usize;
}

struct GenericAgentImpl {
    id: String,
}

#[agent_implementation]
impl GenericAgent<String> for GenericAgentImpl {
    fn new(id: String) -> Self {
        Self { id }
    }

    fn len(&self) -> usize {
        self.id.len()
    }
}

fn main() {}
