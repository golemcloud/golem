use golem_common::base_model::domain_registration::Domain;
use crate::mcp::CompiledMcp;

impl From<CompiledMcp> for golem_api_grpc::proto::golem::mcp::CompiledMcp {
    fn from(value: CompiledMcp) -> Self {
        Self {
            account_id: Some(value.account_id.into()),
            environment_id: Some(value.environment_id.into()),
            deployment_revision: value.deployment_revision.into(),
            domain: value.domain.0,
            agent_types: value.agent_types.into_iter().map(|agent_type| agent_type.to_string()).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::mcp::CompiledMcp> for CompiledMcp {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::mcp::CompiledMcp) -> Result<Self, Self::Error> {
        Ok(Self {
            account_id: value.account_id.ok_or("Missing account_id")?.try_into().map_err(|e| format!("Invalid account_id: {}", e))?,
            environment_id: value.environment_id.ok_or("Missing environment_id")?.try_into().map_err(|e| format!("Invalid environment_id: {}", e))?,
            deployment_revision: value.deployment_revision.try_into().map_err(|e| format!("Invalid deployment_revision: {}", e))?,
            domain: Domain(value.domain),
            agent_types: value.agent_types.into_iter().map(|agent_type| agent_type.parse().map_err(|e| format!("Invalid agent_type '{}': {}", agent_type, e))).collect::<Result<_, _>>()?,
        })
    }
}