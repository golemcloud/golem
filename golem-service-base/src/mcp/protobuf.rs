use crate::mcp::CompiledMcp;
use crate::mcp::CompiledMcpToolExport;
use golem_common::base_model::domain_registration::Domain;
use golem_common::model::account::AccountEmail;
use golem_common::schema::RegisteredAgentTypeSchema;

impl From<CompiledMcpToolExport> for golem_api_grpc::proto::golem::mcp::CompiledMcpToolExport {
    fn from(value: CompiledMcpToolExport) -> Self {
        Self {
            mcp_name: value.mcp_name,
            description: value.description,
            owner_component_id: value.owner_component_id.to_string(),
            owner_component_name: value.owner_component_name.0,
            tool_name: value.tool_name.to_string(),
            command_path: value.command_path,
            definition: Some(value.definition.into()),
            input_schema: Some(value.input_schema.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::mcp::CompiledMcpToolExport> for CompiledMcpToolExport {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::mcp::CompiledMcpToolExport,
    ) -> Result<Self, Self::Error> {
        if value.mcp_name.is_empty() {
            return Err("Invalid MCP tool export metadata".to_string());
        }
        Ok(Self {
            mcp_name: value.mcp_name,
            description: value.description,
            owner_component_id: value
                .owner_component_id
                .parse()
                .map_err(|e| format!("Invalid owner_component_id: {e}"))?,
            owner_component_name: golem_common::model::component::ComponentName(
                value.owner_component_name,
            ),
            tool_name: value.tool_name.parse()?,
            command_path: value.command_path,
            definition: value
                .definition
                .ok_or("Missing tool definition")?
                .try_into()?,
            input_schema: value
                .input_schema
                .ok_or("Missing input schema")?
                .try_into()?,
        })
    }
}

impl From<CompiledMcp> for golem_api_grpc::proto::golem::mcp::CompiledMcp {
    fn from(value: CompiledMcp) -> Self {
        let registered_agent_types = value.registered_agent_types;

        Self {
            account_id: Some(value.account_id.into()),
            account_email: value.account_email.into_inner(),
            environment_id: Some(value.environment_id.into()),
            application_name: value.application_name.0,
            environment_name: value.environment_name.0,
            deployment_revision: value.deployment_revision.into(),
            domain: value.domain.0,
            security_scheme_name: value.security_scheme_name.map(|name| name.0),
            security_scheme: value.security_scheme.map(|s| s.into()),
            registered_agent_types: registered_agent_types
                .into_iter()
                .map(|rat| rat.into())
                .collect(),
            tools: value.tools.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::mcp::CompiledMcp> for CompiledMcp {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::mcp::CompiledMcp,
    ) -> Result<Self, Self::Error> {
        let registered_agent_types: Vec<RegisteredAgentTypeSchema> = value
            .registered_agent_types
            .into_iter()
            .map(|rat| rat.try_into())
            .collect::<Result<_, String>>()?;

        Ok(Self {
            account_id: value
                .account_id
                .ok_or("Missing account_id")?
                .try_into()
                .map_err(|e| format!("Invalid account_id: {}", e))?,
            account_email: AccountEmail::new(value.account_email),
            environment_id: value
                .environment_id
                .ok_or("Missing environment_id")?
                .try_into()
                .map_err(|e| format!("Invalid environment_id: {}", e))?,
            application_name: golem_common::model::application::ApplicationName(
                value.application_name,
            ),
            environment_name: golem_common::model::environment::EnvironmentName(
                value.environment_name,
            ),
            deployment_revision: value
                .deployment_revision
                .try_into()
                .map_err(|e| format!("Invalid deployment_revision: {}", e))?,
            domain: Domain(value.domain),
            security_scheme_name: value
                .security_scheme_name
                .map(golem_common::model::security_scheme::SecuritySchemeName),
            security_scheme: value
                .security_scheme
                .map(|s| s.try_into())
                .transpose()
                .map_err(|e| format!("Invalid security_scheme: {}", e))?,
            registered_agent_types,
            tools: value
                .tools
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}
