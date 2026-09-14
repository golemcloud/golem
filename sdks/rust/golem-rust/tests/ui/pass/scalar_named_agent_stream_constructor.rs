use golem_rust::{FromSchema, IntoSchema, agent_definition};

#[derive(IntoSchema, FromSchema)]
struct AgentStream {
    value: String,
}

#[agent_definition]
trait ScalarNamedAgentStreamConstructorAgent {
    fn new(input: AgentStream) -> Self;
}

fn main() {}
