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

use golem_client::model::{ApiDeployment, ApiSite};
use tracing::info;

use crate::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError};

#[async_trait]
pub trait ApiDeploymentClient {
    async fn deploy(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &ApiDefinitionVersion,
        host: &str,
        subdomain: &str,
    ) -> Result<ApiDeployment, GolemError>;
    async fn list(
        &self,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment>, GolemError>;
    async fn get(&self, site: &str) -> Result<ApiDeployment, GolemError>;
    async fn delete(&self, site: &str) -> Result<String, GolemError>;
}

#[derive(Clone)]
pub struct ApiDeploymentClientLive<C: golem_client::api::ApiDeploymentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ApiDeploymentClient + Sync + Send> ApiDeploymentClient
    for ApiDeploymentClientLive<C>
{
    async fn deploy(
        &self,
        api_definition_id: &ApiDefinitionId,
        version: &ApiDefinitionVersion,
        host: &str,
        subdomain: &str,
    ) -> Result<ApiDeployment, GolemError> {
        info!("Deploying definition {api_definition_id}/{version}, host {host}, subdomain {subdomain}");

        let deployment = ApiDeployment {
            api_definition_id: api_definition_id.0.to_string(),
            version: version.0.to_string(),
            site: ApiSite {
                host: host.to_string(),
                subdomain: subdomain.to_string(),
            },
        };

        Ok(self.client.deploy(&deployment).await?)
    }

    async fn list(
        &self,
        api_definition_id: &ApiDefinitionId,
    ) -> Result<Vec<ApiDeployment>, GolemError> {
        info!("List api deployments with definition {api_definition_id}");

        Ok(self.client.list_deployments(&api_definition_id.0).await?)
    }

    async fn get(&self, site: &str) -> Result<ApiDeployment, GolemError> {
        info!("Getting api deployment for site {site}");

        Ok(self.client.get_deployment(site).await?)
    }

    async fn delete(&self, site: &str) -> Result<String, GolemError> {
        info!("Deleting api deployment for site {site}");

        Ok(self.client.delete_deployment(site).await?)
    }
}
