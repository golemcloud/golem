use golem_rust::agent_definition;

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

struct ReexportedAgentImpl;

impl prelude::ReexportedAgent for ReexportedAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn ping(&self) -> String {
        "ok".to_string()
    }
}

fn main() {}
