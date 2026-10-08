use golem_rust::{FromSchema, IntoSchema, IntoWire, WireSchema, agent_definition};

#[derive(IntoSchema, FromSchema, IntoWire, WireSchema)]
struct AgentStream {
    value: String,
}

#[agent_definition]
trait ScalarNamedAgentStreamConstructorAgent {
    fn new(input: AgentStream) -> Self;
}

fn main() {}
