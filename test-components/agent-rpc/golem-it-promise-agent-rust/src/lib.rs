use golem_rust::{PromiseId, agent_definition, agent_implementation};

#[agent_definition]
pub trait PromiseAgent {
    fn new(name: String) -> Self;
    fn get_promise(&self) -> PromiseId;
    fn await_promise(&self, promise_id: PromiseId);
    fn complete_promise(&self, promise_id: PromiseId, payload_size: u32) -> bool;
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

    fn await_promise(&self, promise_id: PromiseId) {
        let _ = golem_rust::blocking_await_promise(&promise_id);
    }

    fn complete_promise(&self, promise_id: PromiseId, payload_size: u32) -> bool {
        golem_rust::complete_promise(&promise_id, &vec![0; payload_size as usize])
    }
}
