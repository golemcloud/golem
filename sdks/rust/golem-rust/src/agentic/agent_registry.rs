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

// Agent instance at any point in time is only one
thread_local! {
    static AGENT_INSTANCE: RefCell<Option<ResolvedAgent>> = RefCell::new(None);
}

// agent initiator registry is for each agent type name, we have an initiator instance
thread_local! {
    static AGENT_INITIATOR_REGISTRY: Lazy<RefCell<HashMap<AgentTypeName, Box<dyn AgentInitiator>>>> =
    Lazy::new(|| RefCell::new(HashMap::new()));
}

pub fn get_all_agent_types() -> Vec<AgentType> {
    AGENT_TYPE_REGISTRY.with(|registry| registry.borrow().values().cloned().collect())
}

pub fn get_agent_type_by_name(type_name: &str) -> Option<AgentType> {
    let agent_type_name = AgentTypeName(type_name.to_string());
    AGENT_TYPE_REGISTRY.with(|registry| registry.borrow().get(&agent_type_name).cloned())
}

pub fn register_agent_type(type_name: String, agent_type: AgentType) {
    let agent_type_name = AgentTypeName(type_name);
    AGENT_TYPE_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(agent_type_name, agent_type);
        ()
    });
}

pub fn register_agent_instance(resolved_agent: ResolvedAgent) {
    AGENT_INSTANCE.with(|instance| {
        *instance.borrow_mut() = Some(resolved_agent);
    });
}

pub fn with_agent_instance<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&ResolvedAgent) -> R,
{
    AGENT_INSTANCE.with(|instance| instance.borrow().as_ref().map(|agent| f(agent)))
}

pub fn register_agent_initiator(agent_type_name: &str, initiator: Box<dyn AgentInitiator>) {
    let agent_type_name = AgentTypeName(agent_type_name.to_string());
    AGENT_INITIATOR_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(agent_type_name, initiator);
        ()
    });
}

pub fn with_agent_initiator<F, R>(type_name: &AgentTypeName, f: F) -> Option<R>
where
    F: FnOnce(&Box<dyn AgentInitiator>) -> R,
{
    AGENT_INITIATOR_REGISTRY.with(|registry| {
        registry
            .borrow()
            .get(type_name)
            .map(|initiator| f(initiator))
    })
}
