use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
trait Echo {
    fn new() -> Self;
    fn echo(&self, value: String) -> String;
}

struct EchoImpl;

#[agent_implementation]
impl Echo for EchoImpl {
    fn new() -> Self {
        Self
    }

    fn echo(&self, value: String) -> String {
        value
    }
}
