use golem_rust::{agent_definition, agent_implementation};

mod api {
    use super::agent_definition;

    #[agent_definition]
    pub trait ReexportedAgent {
        fn new(id: String) -> Self;
        fn ping(&self) -> String;
    }
}

mod prelude {
    pub use super::api::ReexportedAgent;
}

use api::ReexportedAgent;

struct ReexportedAgentImpl;

#[agent_implementation]
impl prelude::ReexportedAgent for ReexportedAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn ping(&self) -> String {
        "ok".to_string()
    }
}

fn main() {}
