mod protobuf;

use golem_common::base_model::account::AccountId;
use golem_common::base_model::deployment::DeploymentRevision;
use golem_common::base_model::domain_registration::Domain;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
use golem_common::model::component::{ComponentId, ComponentRevision};
use std::collections::HashMap;

use crate::custom_api::SecuritySchemeDetails;

pub type AgentTypeImplementers = HashMap<AgentTypeName, (ComponentId, ComponentRevision)>;

pub struct CompiledMcp {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub domain: Domain,
    pub agent_type_implementers: AgentTypeImplementers,
    pub security_scheme: Option<SecuritySchemeDetails>,
    pub registered_agent_types: Vec<RegisteredAgentType>,
}

impl CompiledMcp {
    pub fn agent_types(&self) -> Vec<AgentTypeName> {
        self.agent_type_implementers.keys().cloned().collect()
    }
}
