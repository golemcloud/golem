use golem_rust::{PromiseId, agent_definition, agent_implementation};

#[agent_definition]
pub trait PromiseAgent {
    fn new(name: String) -> Self;
    fn get_promise(&self) -> PromiseId;
    fn await_promise(&self, promise_id: PromiseId) -> String;
}

struct PromiseAgentImpl {
    _name: String,
}

#[agent_implementation]
impl PromiseAgent for PromiseAgentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn get_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn await_promise(&self, promise_id: PromiseId) -> String {
        String::from_utf8_lossy(&golem_rust::blocking_await_promise(&promise_id)).into_owned()
    }
}
