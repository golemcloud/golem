use golem_rust::agentic::Caller;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait BadAgent {
    fn new(id: String) -> Self;
    fn run(&self, who: Option<Caller>) -> String;
}

struct BadAgentImpl;

#[agent_implementation]
impl BadAgent for BadAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn run(&self, _who: Option<Caller>) -> String {
        String::new()
    }
}

fn main() {}
