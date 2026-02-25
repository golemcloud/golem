use crate::mcp::{AgentTypeImplementers, CompiledMcp};
use golem_common::base_model::domain_registration::Domain;

impl From<CompiledMcp> for golem_api_grpc::proto::golem::mcp::CompiledMcp {
    fn from(value: CompiledMcp) -> Self {
        let agent_type_implementers_json = serde_json::to_string(&value.agent_type_implementers)
            .unwrap_or_else(|_| "{}".to_string());

        Self {
            account_id: Some(value.account_id.into()),
            environment_id: Some(value.environment_id.into()),
            deployment_revision: value.deployment_revision.into(),
            domain: value.domain.0,
            agent_type_implementers: std::collections::HashMap::from([(
                "implementers".to_string(),
                golem_api_grpc::proto::golem::mcp::AgentTypeImplementer {
                    component_id: agent_type_implementers_json,
                    component_revision: 0,
                },
            )]),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::mcp::CompiledMcp> for CompiledMcp {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::mcp::CompiledMcp,
    ) -> Result<Self, Self::Error> {
        // Extract the JSON string from protobuf and deserialize
        let agent_type_implementers_json = value
            .agent_type_implementers
            .get("implementers")
            .map(|impl_info| impl_info.component_id.clone())
            .ok_or("Missing implementers in agent_type_implementers")?;

        let agent_type_implementers: AgentTypeImplementers =
            serde_json::from_str(&agent_type_implementers_json)
                .map_err(|e| format!("Failed to parse agent_type_implementers: {}", e))?;

        Ok(Self {
            account_id: value
                .account_id
                .ok_or("Missing account_id")?
                .try_into()
                .map_err(|e| format!("Invalid account_id: {}", e))?,
            environment_id: value
                .environment_id
                .ok_or("Missing environment_id")?
                .try_into()
                .map_err(|e| format!("Invalid environment_id: {}", e))?,
            deployment_revision: value
                .deployment_revision
                .try_into()
                .map_err(|e| format!("Invalid deployment_revision: {}", e))?,
            domain: Domain(value.domain),
            agent_type_implementers,
        })
    }
}
