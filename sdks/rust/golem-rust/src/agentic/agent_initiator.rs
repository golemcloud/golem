use crate::golem_agentic::{
    exports::golem::agent::guest::DataValue, golem::agent::common::AgentError,
};

/**
 * An initiator for an agent implementation.
 * This is used to create an instance of the agent with the given parameters.
 */
pub trait AgentInitiator {
    fn initiate(&self, params: DataValue) -> Result<(), AgentError>;
}
