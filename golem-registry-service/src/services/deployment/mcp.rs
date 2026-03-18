use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;
use crate::services::security_scheme::SecuritySchemeService;
use golem_common::base_model::domain_registration::Domain;
use golem_common::model::agent::RegisteredAgentType;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::model::auth::AuthCtx;
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
    security_scheme_service: Arc<SecuritySchemeService>,
}

impl DeployedMcpService {
    pub fn new(
        deployment_repo: Arc<dyn DeploymentRepo>,
        security_scheme_service: Arc<SecuritySchemeService>,
    ) -> Self {
        Self {
            deployment_repo,
            security_scheme_service,
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
                let security_scheme_id = deployment.mcp_data.value().security_scheme_id;
                let environment_id = deployment.environment_id;
                let mut compiled_mcp = CompiledMcp::try_from(deployment)?;

                if let Some(scheme_id) = security_scheme_id {
                    let scheme = self
                        .security_scheme_service
                        .get(scheme_id, &AuthCtx::system())
                        .await
                        .ok();

                    compiled_mcp.security_scheme = scheme.map(|s| SecuritySchemeDetails {
                        id: s.id,
                        name: s.name,
                        provider_type: s.provider_type,
                        client_id: s.client_id,
                        client_secret: s.client_secret,
                        redirect_url: s.redirect_url,
                        scopes: s.scopes,
                    });
                }

                let mut registered_agent_types = Vec::new();
                for (agent_type_name, _) in &compiled_mcp.agent_type_implementers {
                    match self
                        .deployment_repo
                        .get_deployed_agent_type(environment_id, &agent_type_name.0)
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
