use golem_rust::agent_definition;

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

impl AgentAlias for AliasedAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn ping(&self) -> String {
        "ok".to_string()
    }
}

fn main() {}
