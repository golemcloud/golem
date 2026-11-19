use golem_rust::agentic::Principal;
use golem_rust::{tool_definition, tool_implementation};

#[tool_definition]
trait BadTool {
    fn run(&self, who: Principal) -> String;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _who: Principal) -> String {
        String::new()
    }
}

fn main() {}
