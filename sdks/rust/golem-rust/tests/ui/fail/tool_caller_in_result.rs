use golem_rust::agentic::Caller;
use golem_rust::{tool_definition, tool_implementation};

#[tool_definition]
trait BadTool {
    fn run(&self, name: String) -> Caller;
}

struct BadToolImpl;

#[tool_implementation]
impl BadTool for BadToolImpl {
    fn run(&self, _name: String) -> Caller {
        unimplemented!()
    }
}

fn main() {}
