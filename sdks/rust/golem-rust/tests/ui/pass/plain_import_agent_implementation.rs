use golem_rust::{agent_definition, agent_implementation};

mod api {
    use super::agent_definition;

    #[agent_definition]
    pub trait ImportedAgent {
        fn new(id: String) -> Self;
        fn ping(&self) -> String;
    }
}

use api::ImportedAgent;

struct ImportedAgentImpl;

#[agent_implementation]
impl ImportedAgent for ImportedAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn ping(&self) -> String {
        "ok".to_string()
    }
}

fn main() {}
