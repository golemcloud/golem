use crate::model::security_scheme::SecurityScheme;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;
use crate::repo::security_scheme::SecuritySchemeRepo;
use golem_common::base_model::domain_registration::Domain;
use golem_common::model::agent::RegisteredAgentType;
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
                    match self
                        .deployment_repo
                        .get_deployed_agent_type(compiled_mcp.environment_id.0, &agent_type_name.0)
                        .await
                    {
                        Ok(Some(record)) => {
                            match golem_common::model::agent::DeployedRegisteredAgentType::try_from(
                                record,
                            ) {
                                Ok(deployed) => {
                                    registered_agent_types
                                        .push(RegisteredAgentType::from(deployed));
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to convert agent type {} for domain {}: {}",
                                        agent_type_name.0,
                                        domain.0,
                                        e
                                    );
                                }
                            }
                        }
                        Ok(None) => {
                            tracing::warn!(
                                "Agent type {} not found for domain {}",
                                agent_type_name.0,
                                domain.0,
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to fetch agent type {} for domain {}: {}",
                                agent_type_name.0,
                                domain.0,
                                e
                            );
                        }
                    }
                }
                compiled_mcp.registered_agent_types = registered_agent_types;

                Ok(compiled_mcp)
            }
            None => Err(DeployedMcpError::NoActiveMcpForDomain(domain.clone())),
        }
    }
}
