use golem_rust::agentic::TokenProvenance;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait BadAgent {
    fn new(token: TokenProvenance) -> Self;
    fn run(&self) -> String;
}

struct BadAgentImpl;

#[agent_implementation]
impl BadAgent for BadAgentImpl {
    fn new(_token: TokenProvenance) -> Self {
        Self
    }

    fn run(&self) -> String {
        String::new()
    }
}

fn main() {}
