mod protobuf;


use golem_common::base_model::account::AccountId;
use golem_common::base_model::deployment::DeploymentRevision;
use golem_common::base_model::domain_registration::Domain;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use std::collections::HashMap;

/// Maps agent type names to their implementing component versions
pub type AgentTypeImplementers = HashMap<AgentTypeName, (ComponentId, ComponentRevision)>;

pub struct CompiledMcp {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub domain: Domain,
    pub agent_type_implementers: AgentTypeImplementers,
}

impl CompiledMcp {
    pub fn agent_types(&self) -> Vec<AgentTypeName> {
        self.agent_type_implementers.keys().cloned().collect()
    }
}

