use golem_rust::agentic::Principal;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait BadAgent {
    fn new(id: String) -> Self;
    fn run(&self) -> Option<Principal>;
}

struct BadAgentImpl;

#[agent_implementation]
impl BadAgent for BadAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn run(&self) -> Option<Principal> {
        None
    }
}

fn main() {}
