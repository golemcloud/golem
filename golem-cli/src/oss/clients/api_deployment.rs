// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use itertools::Itertools;

use crate::clients::api_deployment::ApiDeploymentClient;
use golem_client::model::{ApiDefinitionInfo, ApiSite};
use tracing::info;

use crate::model::{ApiDefinitionId, ApiDefinitionIdWithVersion, ApiDeployment, GolemError};
use crate::oss::model::OssContext;

#[derive(Clone)]
pub struct ApiDeploymentClientLive<C: golem_client::api::ApiDeploymentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ApiDeploymentClient + Sync + Send> ApiDeploymentClient
    for ApiDeploymentClientLive<C>
{
    type ProjectContext = OssContext;

    async fn deploy(
        &self,
        definitions: Vec<ApiDefinitionIdWithVersion>,
        host: &str,
        subdomain: Option<String>,
        _project: &Self::ProjectContext,
    ) -> Result<ApiDeployment, GolemError> {
        info!(
            "Deploying definitions to host {host} {}",
            subdomain
                .clone()
                .map_or("".to_string(), |s| format!("subdomain {}", s))
        );

        let api_definition_infos = definitions
            .iter()
            .map(|d| ApiDefinitionInfo {
                id: d.id.0.clone(),
                version: d.version.0.clone(),
            })
            .collect::<Vec<_>>();

        let deployment = golem_client::model::ApiDeploymentRequest {
            api_definitions: api_definition_infos,
            site: ApiSite {
                host: host.to_string(),
                subdomain,
            },
        };

        Ok(self.client.deploy(&deployment).await?.into())
    }

    async fn list(
        &self,
        api_definition_id: &ApiDefinitionId,
        _project: &Self::ProjectContext,
    ) -> Result<Vec<ApiDeployment>, GolemError> {
        info!("List api deployments with definition {api_definition_id}");

        let deployments = self.client.list_deployments(&api_definition_id.0).await?;

        Ok(deployments.into_iter().map_into().collect())
    }

    async fn get(&self, site: &str) -> Result<ApiDeployment, GolemError> {
        info!("Getting api deployment for site {site}");

        Ok(self.client.get_deployment(site).await?.into())
    }

    async fn delete(&self, site: &str) -> Result<String, GolemError> {
        info!("Deleting api deployment for site {site}");

        Ok(self.client.delete_deployment(site).await?)
    }
}
