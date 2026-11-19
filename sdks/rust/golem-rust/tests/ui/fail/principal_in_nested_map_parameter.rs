use golem_rust::agentic::Principal;
use golem_rust::{agent_definition, agent_implementation};
use std::collections::HashMap;

#[agent_definition]
trait BadAgent {
    fn new(id: String) -> Self;
    fn run(&self, who: HashMap<String, Principal>) -> String;
}

struct BadAgentImpl;

#[agent_implementation]
impl BadAgent for BadAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn run(&self, _who: HashMap<String, Principal>) -> String {
        String::new()
    }
}

fn main() {}
