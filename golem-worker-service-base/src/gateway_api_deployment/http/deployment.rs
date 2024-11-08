use crate::gateway_api_deployment::http::ApiSite;
use crate::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use std::fmt::Debug;

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
