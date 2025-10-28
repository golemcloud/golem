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

// use crate::gateway_api_definition::{ApiDefinitionId, ApiVersion};
// use crate::gateway_api_deployment::ApiSite;
use golem_common::model::worker::FlatWorkerMetadata;
use golem_common::model::ScanCursor;
use poem_openapi::Object;
use std::fmt::Debug;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Object)]
pub struct WorkersMetadataResponse {
    pub workers: Vec<FlatWorkerMetadata>,
    pub cursor: Option<ScanCursor>,
}

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
// #[serde(rename_all = "camelCase")]
// #[oai(rename_all = "camelCase")]
// pub struct ApiDeploymentRequest {
//     pub api_definitions: Vec<ApiDefinitionInfo>,
//     pub project_id: ProjectId,
//     pub site: ApiSite,
// }

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
// #[serde(rename_all = "camelCase")]
// #[oai(rename_all = "camelCase")]
// pub struct ApiDeployment {
//     pub api_definitions: Vec<ApiDefinitionInfo>,
//     pub project_id: ProjectId,
//     pub site: ApiSite,
//     pub created_at: Option<chrono::DateTime<chrono::Utc>>,
// }

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
// #[serde(rename_all = "camelCase")]
// #[oai(rename_all = "camelCase")]
// pub struct ApiDefinitionInfo {
//     pub id: ApiDefinitionId,
//     pub version: ApiVersion,
// }

// impl From<crate::gateway_api_deployment::ApiDeployment> for ApiDeployment {
//     fn from(api_deployment: crate::gateway_api_deployment::ApiDeployment) -> Self {
//         Self {
//             api_definitions: api_deployment
//                 .api_definition_keys
//                 .iter()
//                 .map(|k| ApiDefinitionInfo {
//                     id: k.id.clone(),
//                     version: k.version.clone(),
//                 })
//                 .collect(),
//             project_id: api_deployment.namespace.project_id.clone(),
//             site: api_deployment.site.clone(),
//             created_at: Some(api_deployment.created_at),
//         }
//     }
// }

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
// #[serde(rename_all = "camelCase")]
// #[oai(rename_all = "camelCase")]
// pub struct ApiDomain {
//     pub project_id: ProjectId,
//     pub domain_name: String,
//     pub name_servers: Vec<String>,
//     pub created_at: Option<chrono::DateTime<chrono::Utc>>,
// }

// impl ApiDomain {
//     pub fn new(
//         request: &DomainRequest,
//         name_servers: Vec<String>,
//         created_at: chrono::DateTime<chrono::Utc>,
//     ) -> Self {
//         Self {
//             project_id: request.project_id.clone(),
//             domain_name: request.domain_name.clone(),
//             name_servers,
//             created_at: Some(created_at),
//         }
//     }
// }

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// #[serde(rename_all = "camelCase")]
// pub struct AccountApiDomain {
//     pub account_id: AccountId,
//     pub domain: ApiDomain,
// }

// impl AccountApiDomain {
//     pub fn new(account_id: &AccountId, domain: &ApiDomain) -> Self {
//         Self {
//             account_id: account_id.clone(),
//             domain: domain.clone(),
//         }
//     }
// }

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
// #[serde(rename_all = "camelCase")]
// #[oai(rename_all = "camelCase")]
// pub struct DomainRequest {
//     pub project_id: ProjectId,
//     pub domain_name: String,
// }

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
// #[serde(rename_all = "camelCase")]
// #[oai(rename_all = "camelCase")]
// pub struct CertificateRequest {
//     pub project_id: ProjectId,
//     pub domain_name: String,
//     pub certificate_body: String,
//     pub certificate_private_key: String,
// }

// #[derive(Debug, Clone, Eq, PartialEq, Hash, FromStr, Serialize, Deserialize, NewType)]
// pub struct CertificateId(pub Uuid);

// impl Display for CertificateId {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.0)
//     }
// }

// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
// #[serde(rename_all = "camelCase")]
// #[oai(rename_all = "camelCase")]
// pub struct Certificate {
//     pub id: CertificateId,
//     pub project_id: ProjectId,
//     pub domain_name: String,
//     pub created_at: Option<chrono::DateTime<chrono::Utc>>,
// }

// impl Certificate {
//     pub fn new(request: &CertificateRequest, created_at: chrono::DateTime<chrono::Utc>) -> Self {
//         Self {
//             id: CertificateId(Uuid::new_v4()),
//             project_id: request.project_id.clone(),
//             domain_name: request.domain_name.clone(),
//             created_at: Some(created_at),
//         }
//     }
// }
