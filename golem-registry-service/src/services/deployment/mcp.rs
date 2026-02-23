use std::sync::Arc;
use golem_common::base_model::domain_registration::Domain;
use golem_common::{error_forwarding, SafeDisplay};
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::repo::RepoError;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::deployment::DeployRepoError;

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


error_forwarding!(
    DeployedMcpError,
    RepoError,
    DeployRepoError

);

pub struct DeployedMcpService {
    deployment_repo: Arc<dyn DeploymentRepo>,
}

impl DeployedMcpService {
    pub fn new(deployment_repo: Arc<dyn DeploymentRepo>) -> Self {
        Self { deployment_repo }
    }
    
    pub async fn get_currently_active_mcp(
        &self,
        domain: &Domain,
    ) ->Result<CompiledMcp, DeployedMcpError> {
        let optional_deployment =
            self.deployment_repo.get_active_mcp_for_domain(&domain.0).await?;
        
        
        match optional_deployment {
            Some(deployment) => {
                let compiled_mcp = CompiledMcp::try_from(deployment)?;

                Ok(compiled_mcp)
            },
            None => Err(DeployedMcpError::NoActiveMcpForDomain(domain.clone())),
        }

    }
}