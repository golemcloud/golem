use golem_rust::agentic::Caller;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait BadAgent {
    fn new(id: String) -> Self;
    fn run(&self) -> Caller;
}

struct BadAgentImpl;

#[agent_implementation]
impl BadAgent for BadAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn run(&self) -> Caller {
        Caller::anonymous()
    }
}

fn main() {}
