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

use crate::clients::api_deployment::ApiDeploymentClient;
use crate::model::{ApiDefinitionId, ApiDefinitionIdWithVersion, GolemError, GolemResult};
use async_trait::async_trait;

#[async_trait]
pub trait ApiDeploymentService {
    type ProjectContext;

    async fn deploy(
        &self,
        definitions: Vec<ApiDefinitionIdWithVersion>,
        host: String,
        subdomain: Option<String>,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError>;
    async fn get(&self, site: String) -> Result<GolemResult, GolemError>;
    async fn list(
        &self,
        id: ApiDefinitionId,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(&self, site: String) -> Result<GolemResult, GolemError>;
}

pub struct ApiDeploymentServiceLive<ProjectContext> {
    pub client: Box<dyn ApiDeploymentClient<ProjectContext = ProjectContext> + Send + Sync>,
}

#[async_trait]
impl<ProjectContext: Send + Sync> ApiDeploymentService
    for ApiDeploymentServiceLive<ProjectContext>
{
    type ProjectContext = ProjectContext;

    async fn deploy(
        &self,
        definitions: Vec<ApiDefinitionIdWithVersion>,
        host: String,
        subdomain: Option<String>,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError> {
        let deployment = self
            .client
            .deploy(definitions, &host, subdomain, project)
            .await?;

        Ok(GolemResult::Ok(Box::new(deployment)))
    }

    async fn get(&self, site: String) -> Result<GolemResult, GolemError> {
        let deployment = self.client.get(&site).await?;

        Ok(GolemResult::Ok(Box::new(deployment)))
    }

    async fn list(
        &self,
        id: ApiDefinitionId,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError> {
        let deployments = self.client.list(&id, project).await?;

        Ok(GolemResult::Ok(Box::new(deployments)))
    }

    async fn delete(&self, site: String) -> Result<GolemResult, GolemError> {
        let res = self.client.delete(&site).await?;

        Ok(GolemResult::Str(res))
    }
}
