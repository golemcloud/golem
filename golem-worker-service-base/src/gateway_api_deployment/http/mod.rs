// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::service::gateway::api_definition::ApiDefinitionIdWithVersion;
use poem_openapi::{NewType, Object};
use serde::{Deserialize, Serialize};
use std::fmt::Display;

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
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
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
