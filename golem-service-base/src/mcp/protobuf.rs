use crate::mcp::{AgentTypeImplementers, CompiledMcp};
use golem_common::base_model::domain_registration::Domain;
use golem_common::model::agent::AgentTypeName;

impl From<CompiledMcp> for golem_api_grpc::proto::golem::mcp::CompiledMcp {
    fn from(value: CompiledMcp) -> Self {
        Self {
            account_id: Some(value.account_id.into()),
            environment_id: Some(value.environment_id.into()),
            deployment_revision: value.deployment_revision.into(),
            domain: value.domain.0,
            agent_type_implementers: value
                .agent_type_implementers
                .into_iter()
                .map(|(name, (component_id, component_revision))| {
                    (
                        name.0,
                        golem_api_grpc::proto::golem::registry::RegisteredAgentTypeImplementer {
                            component_id: Some(component_id.into()),
                            component_revision: component_revision.into(),
                        },
                    )
                })
                .collect(),
            security_scheme: value.security_scheme.map(|s| s.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::mcp::CompiledMcp> for CompiledMcp {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::mcp::CompiledMcp,
    ) -> Result<Self, Self::Error> {
        let agent_type_implementers: AgentTypeImplementers = value
            .agent_type_implementers
            .into_iter()
            .map(|(name, implementer)| {
                let component_id = implementer
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()
                    .map_err(|e| format!("Invalid component_id: {}", e))?;
                let component_revision = implementer
                    .component_revision
                    .try_into()
                    .map_err(|e| format!("Invalid component_revision: {}", e))?;
                Ok((AgentTypeName(name), (component_id, component_revision)))
            })
            .collect::<Result<_, String>>()?;

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
            security_scheme: value
                .security_scheme
                .map(|s| s.try_into())
                .transpose()
                .map_err(|e| format!("Invalid security_scheme: {}", e))?,
        })
    }
}
