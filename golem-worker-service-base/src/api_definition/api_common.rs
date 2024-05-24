use std::fmt::Debug;
use std::fmt::Display;

use crate::service::api_definition::ApiDefinitionKey;
use bincode::{Decode, Encode};
use poem_openapi::NewType;
use serde::{Deserialize, Serialize};

use crate::worker_binding::GolemWorkerBinding;
use poem_openapi::Object;

// Common to API definitions regardless of different protocols
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiDefinitionId(pub String);

impl Display for ApiDefinitionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
pub struct ApiVersion(pub String);

impl Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// Constraints applicable to any type of API Definition
pub trait HasApiDefinitionId {
    fn get_api_definition_id(&self) -> ApiDefinitionId;
}

pub trait HasVersion {
    fn get_version(&self) -> ApiVersion;
}

pub trait HasIsDraft {
    // Editable. Can't be published.
    fn is_draft(&self) -> bool;
    fn set_not_draft(&mut self);
}

pub trait HasGolemWorkerBindings {
    fn get_golem_worker_bindings(&self) -> Vec<GolemWorkerBinding>;
}

#[derive(
    Eq, Hash, PartialEq, Clone, Debug, serde::Deserialize, bincode::Encode, bincode::Decode,
)]
pub struct ApiDeployment<Namespace> {
    pub api_definition_id: ApiDefinitionKey<Namespace>,
    pub site: ApiSite,
}

#[derive(Debug, Eq, Clone, Hash, PartialEq, Serialize, Deserialize, Encode, Decode, Object)]
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

#[derive(PartialEq, Eq, Clone, Debug, Hash, Serialize, Deserialize, Encode, Decode, NewType)]
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
