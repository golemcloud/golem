use golem_common::base_model::domain_registration::Domain;
use crate::mcp::{CompiledMcp, AgentTypeImplementers};

// Helper to construct ComponentRevision from u64 since the field is pub(crate)
fn component_revision_from_u64(value: i64) -> golem_common::model::component::ComponentRevision {
    // The revision is a newtype over u64, but the constructor is pub(crate)
    // We need to use a workaround - manually construct using the internal representation
    // For now, we'll use unsafe since the field is pub(crate) u64
    unsafe {
        std::mem::transmute::<u64, golem_common::model::component::ComponentRevision>(value as u64)
    }
}

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
                .map(|(name, (component_id, revision))| {
                    (
                        name.0,
                        golem_api_grpc::proto::golem::mcp::AgentTypeImplementer {
                            component_id: component_id.0.to_string(),
                            component_revision: unsafe {
                                std::mem::transmute::<golem_common::model::component::ComponentRevision, u64>(revision) as i64
                            },
                        },
                    )
                })
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::mcp::CompiledMcp> for CompiledMcp {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::mcp::CompiledMcp) -> Result<Self, Self::Error> {
        use golem_common::model::agent::AgentTypeName;
        use golem_common::model::component::ComponentId;
        use uuid::Uuid;
        
        let mut agent_type_implementers = AgentTypeImplementers::new();
        
        for (name, impl_info) in value.agent_type_implementers {
            let agent_type_name = AgentTypeName(name);
            let component_id = ComponentId(
                Uuid::parse_str(&impl_info.component_id)
                    .map_err(|e| format!("Invalid component_id: {}", e))?
            );
            let component_revision = component_revision_from_u64(impl_info.component_revision);
            agent_type_implementers.insert(agent_type_name, (component_id, component_revision));
        }
        
        Ok(Self {
            account_id: value.account_id.ok_or("Missing account_id")?.try_into().map_err(|e| format!("Invalid account_id: {}", e))?,
            environment_id: value.environment_id.ok_or("Missing environment_id")?.try_into().map_err(|e| format!("Invalid environment_id: {}", e))?,
            deployment_revision: value.deployment_revision.try_into().map_err(|e| format!("Invalid deployment_revision: {}", e))?,
            domain: Domain(value.domain),
            agent_type_implementers,
        })
    }
}