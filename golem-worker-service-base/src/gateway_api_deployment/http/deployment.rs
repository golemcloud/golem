use std::fmt::Debug;
use std::fmt::Display;

use crate::gateway_api_deployment::ApiSite;
use crate::service::gateway::api_definition::ApiDefinitionIdWithVersion;

#[derive(Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize)]
pub struct ApiDeploymentRequest<Namespace> {
    pub namespace: Namespace,
    pub api_definition_keys: Vec<ApiDefinitionIdWithVersion>,
    pub site: ApiSite,
}

#[derive(Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize)]
pub struct ApiDeployment<Namespace> {
    pub namespace: Namespace,
    pub api_definition_keys: Vec<ApiDefinitionIdWithVersion>,
    pub site: ApiSite,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

