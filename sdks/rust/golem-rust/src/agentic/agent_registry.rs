use crate::{
    agentic::{agent_initiator::AgentInitiator, agent_type_name::AgentTypeName, ResolvedAgent},
    golem_agentic::exports::golem::agent::guest::AgentType,
};

// The registry should hold the agent definitions available in this module
// TODO; Implement registration of definitions and retrieval
pub fn get_all_agent_definitions() -> Vec<AgentType> {
    todo!("Unimplemented function to get all agent definitions")
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
