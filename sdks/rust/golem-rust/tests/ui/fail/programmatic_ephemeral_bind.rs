use golem_rust::{AgentClientDefinition, ParsedAgentId};

fn main() {
    let definition = AgentClientDefinition::builder()
        .ephemeral::<String>("EphemeralAgent")
        .build();
    let id = ParsedAgentId::new("EphemeralAgent()".to_string());
    let _ = definition.bind(&id);
}
