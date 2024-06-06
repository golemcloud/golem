// Copyright 2024 Golem Cloud
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

use crate::clients::api_deployment::ApiDeploymentClient;
use crate::cloud::model::ProjectId;
use golem_cloud_client::model::ApiSite;
use itertools::Itertools;
use tracing::info;

use crate::model::{ApiDefinitionId, ApiDefinitionIdWithVersion, ApiDeployment, GolemError};

#[derive(Clone)]
pub struct ApiDeploymentClientLive<C: golem_cloud_client::api::ApiDeploymentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ApiDeploymentClient + Sync + Send> ApiDeploymentClient
    for ApiDeploymentClientLive<C>
{
    type ProjectContext = ProjectId;

    async fn deploy(
        &self,
        api_definitions: Vec<ApiDefinitionIdWithVersion>,
        host: &str,
        subdomain: Option<String>,
        project: &Self::ProjectContext,
    ) -> Result<ApiDeployment, GolemError> {
        info!(
            "Deploying definitions to host {host} {}",
            subdomain
                .clone()
                .map_or("".to_string(), |s| format!("subdomain {}", s))
        );

        if api_definitions.len() > 1 {
            Err(GolemError(
                "Multiple API definitions in a deployment is not supported in Golem Cloud yet"
                    .to_string(),
            ))
        } else {
            let api_definition_id = api_definitions[0].id.0.clone();
            let version = api_definitions[0].version.0.clone();
            let deployment = golem_cloud_client::model::ApiDeployment {
                api_definition_id,
                version,
                project_id: project.0,
                site: ApiSite {
                    host: host.to_string(),
                    subdomain: subdomain.expect("Subdomain is mandatory"), // TODO: unify OSS and cloud
                },
            };

            Ok(self.client.deploy(&deployment).await?.into())
        }
    }

    async fn list(
        &self,
        api_definition_id: &ApiDefinitionId,
        project: &Self::ProjectContext,
    ) -> Result<Vec<ApiDeployment>, GolemError> {
        info!("List api deployments with definition {api_definition_id}");

        let deployments = self
            .client
            .list_deployments(&project.0, &api_definition_id.0)
            .await?;

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
