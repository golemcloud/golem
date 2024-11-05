use std::fmt::Debug;
use std::fmt::Display;

use serde::{Deserialize, Serialize};

use poem_openapi::{NewType, Object};
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

#[derive(Debug, Eq, Clone, Hash, PartialEq, Serialize, Deserialize, Object)]
pub struct ApiSite {
    pub host: String,
    pub subdomain: Option<String>,
}

impl Display for ApiSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Need to see how to remove the need of subdomain for localhost , as subdomains are not allowed for localhost
        match &self.subdomain {
            Some(subdomain) => write!(f, "{}.{}", subdomain, self.host),
            None => write!(f, "{}", self.host),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Hash, Serialize, Deserialize, NewType)]
pub struct ApiSiteString(pub String);

impl From<&ApiSite> for ApiSiteString {
    fn from(value: &ApiSite) -> Self {
        ApiSiteString(value.to_string())
    }
}

impl Display for ApiSiteString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
