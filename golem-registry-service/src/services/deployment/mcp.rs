use crate::model::security_scheme::SecurityScheme;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;
use crate::repo::security_scheme::SecuritySchemeRepo;
use golem_common::base_model::domain_registration::Domain;
use golem_common::model::agent::RegisteredAgentType;
use golem_common::schema::RegisteredAgentTypeSchema;
use golem_common::schema::adapters::agent_type_to_schema;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum DeployedMcpError {
    #[error("No active mcp capabilities for domain {0} found")]
    NoActiveMcpForDomain(Domain),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for DeployedMcpError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::NoActiveMcpForDomain(_) => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(DeployedMcpError, RepoError, DeployRepoError);

pub struct DeployedMcpService {
    deployment_repo: Arc<dyn DeploymentRepo>,
    security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
}

impl DeployedMcpService {
    pub fn new(
        deployment_repo: Arc<dyn DeploymentRepo>,
        security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
    ) -> Self {
        Self {
            deployment_repo,
            security_scheme_repo,
        }
    }

    pub async fn get_currently_active_mcp(
        &self,
        domain: &Domain,
    ) -> Result<CompiledMcp, DeployedMcpError> {
        let optional_deployment = self
            .deployment_repo
            .get_active_mcp_for_domain(&domain.0)
            .await?;

        match optional_deployment {
            Some(deployment) => {
                let mut compiled_mcp = CompiledMcp::try_from(deployment)?;

                // Resolve security scheme by name at runtime
                if let Some(scheme_name) = &compiled_mcp.security_scheme_name {
                    let scheme_record = self
                        .security_scheme_repo
                        .get_for_environment_and_name(compiled_mcp.environment_id.0, &scheme_name.0)
                        .await
                        .ok()
                        .flatten();

                    if let Some(record) = scheme_record
                        && let Ok(scheme) = SecurityScheme::try_from(record)
                    {
                        compiled_mcp.security_scheme = Some(SecuritySchemeDetails {
                            id: scheme.id,
                            name: scheme.name,
                            provider_type: scheme.provider_type,
                            client_id: scheme.client_id,
                            client_secret: scheme.client_secret,
                            redirect_url: scheme.redirect_url,
                            scopes: scheme.scopes,
                        });
                    }
                }

                let mut registered_agent_types = Vec::new();
                for agent_type_name in compiled_mcp.agent_type_implementers.keys() {
                    // Fail loudly on any inconsistency: an active MCP deployment
                    // must advertise the full set of agent types it claims, or
                    // none at all. Silently dropping a type would expose a
                    // partial/empty capability set and hide conversion
                    // regressions during the schema cutover.
                    let record = self
                        .deployment_repo
                        .get_deployed_agent_type(compiled_mcp.environment_id.0, &agent_type_name.0)
                        .await?
                        .ok_or_else(|| {
                            DeployedMcpError::InternalError(anyhow::anyhow!(
                                "Agent type {} not found for domain {}",
                                agent_type_name.0,
                                domain.0,
                            ))
                        })?;

                    let deployed =
                        golem_common::model::agent::DeployedRegisteredAgentType::try_from(record)
                            .map_err(|e| {
                            DeployedMcpError::InternalError(anyhow::anyhow!(
                                "Failed to convert agent type {} for domain {}: {}",
                                agent_type_name.0,
                                domain.0,
                                e
                            ))
                        })?;

                    let legacy = RegisteredAgentType::from(deployed);
                    let agent_type = agent_type_to_schema(&legacy.agent_type).map_err(|e| {
                        DeployedMcpError::InternalError(anyhow::anyhow!(
                            "Failed to convert agent type {} to schema model for domain {}: {}",
                            agent_type_name.0,
                            domain.0,
                            e
                        ))
                    })?;

                    registered_agent_types.push(RegisteredAgentTypeSchema {
                        agent_type,
                        implemented_by: legacy.implemented_by,
                    });
                }
                compiled_mcp.registered_agent_types = registered_agent_types;

                Ok(compiled_mcp)
            }
            None => Err(DeployedMcpError::NoActiveMcpForDomain(domain.clone())),
        }
    }
}
