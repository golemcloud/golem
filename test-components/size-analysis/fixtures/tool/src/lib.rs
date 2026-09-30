use golem_rust::{tool_definition, tool_implementation};

#[tool_definition(version = "1.0.0")]
trait EchoTool {
    fn echo(&self, value: String) -> String;
}

struct EchoToolImpl;

#[tool_implementation]
impl EchoTool for EchoToolImpl {
    fn echo(&self, value: String) -> String {
        value
    }
}
