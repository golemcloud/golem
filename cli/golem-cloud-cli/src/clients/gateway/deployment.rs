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
use golem_cloud_worker_client::model::ApiDeployment;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait DeploymentClient {
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<Vec<ApiDeployment>, GolemError>;
    async fn update(&self, api_deployment: ApiDeployment) -> Result<ApiDeployment, GolemError>;
    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
        site: &str,
    ) -> Result<String, GolemError>;
}

pub struct DeploymentClientLive<
    C: golem_cloud_worker_client::api::ApiDeploymentClient + Sync + Send,
> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_worker_client::api::ApiDeploymentClient + Sync + Send> DeploymentClient
    for DeploymentClientLive<C>
{
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
    ) -> Result<Vec<ApiDeployment>, GolemError> {
        Ok(self
            .client
            .list_deployments(&project_id.0, api_definition_id)
            .await?)
    }

    async fn update(&self, api_deployment: ApiDeployment) -> Result<ApiDeployment, GolemError> {
        Ok(self.client.deploy(&api_deployment).await?)
    }

    async fn delete(
        &self,
        _project_id: ProjectId,
        _api_definition_id: &str,
        site: &str,
    ) -> Result<String, GolemError> {
        Ok(self.client.delete_deployment(site).await?)
    }
}
