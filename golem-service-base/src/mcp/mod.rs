mod protobuf;


use golem_common::base_model::account::AccountId;
use golem_common::base_model::deployment::DeploymentRevision;
use golem_common::base_model::domain_registration::Domain;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::AgentTypeName;

pub struct CompiledMcp {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub domain: Domain,
    pub agent_types: Vec<AgentTypeName>
}

