use golem_rust::{agent_definition, agent_implementation};

mod api {
    use super::agent_definition;

    #[agent_definition]
    pub trait AliasedAgent {
        fn new(id: String) -> Self;
        fn ping(&self) -> String;
    }
}

use api::AliasedAgent as AgentAlias;

struct AliasedAgentImpl;

#[agent_implementation]
impl AgentAlias for AliasedAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn ping(&self) -> String {
        "ok".to_string()
    }
}

fn main() {}
