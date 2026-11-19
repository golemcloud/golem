use golem_rust::agentic::Principal;
use golem_rust::{tool_definition, tool_implementation};
use std::collections::HashMap;

#[tool_definition]
trait BadTool {
    fn run(&self, who: HashMap<String, Principal>) -> String;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _who: HashMap<String, Principal>) -> String {
        String::new()
    }
}

fn main() {}
