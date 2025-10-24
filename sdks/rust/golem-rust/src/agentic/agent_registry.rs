use once_cell::unsync::Lazy;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::{
    agentic::{agent_initiator::AgentInitiator, agent_type_name::AgentTypeName, ResolvedAgent},
    golem_agentic::exports::golem::agent::guest::AgentType,
};

thread_local! {
    static AGENT_TYPE_REGISTRY: Lazy<RefCell<HashMap<AgentTypeName, AgentType>>> =
    Lazy::new(|| RefCell::new(HashMap::new()));
}

pub fn get_all_agent_types() -> Vec<AgentType> {
    AGENT_TYPE_REGISTRY.with(|registry| registry.borrow().values().cloned().collect())
}

pub fn register_agent_type(type_name: String, agent_type: AgentType) {
    let agent_type_name = AgentTypeName(type_name);
    AGENT_TYPE_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(agent_type_name, agent_type);
        ()
    });
}

// The registry should hold the initiator instances for each agent type
// TODO; Implement registration of initiators and retrieval
pub fn get_agent_initiator(agent_type_name: &AgentTypeName) -> Option<Box<dyn AgentInitiator>> {
    todo!(
        "Unimplemented function to get agent initiator of type {}",
        agent_type_name.0
    )
}

// At any point, there should be only one active agent instance
pub fn get_agent_instance() -> Option<ResolvedAgent> {
    todo!("Unimplemented function to get the active agent instance")
}
