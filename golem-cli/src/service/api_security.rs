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

use crate::clients::api_security::ApiSecurityClient;
use crate::model::{GolemError, GolemResult};
use async_trait::async_trait;
use golem_client::model::Provider;

#[async_trait]
pub trait ApiSecuritySchemeService {
    type ProjectContext;

    async fn create(
        &self,
        id: String,
        provider_type: Provider,
        client_id: String,
        client_secret: String,
        scope: Vec<String>,
        redirect_url: String,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError>;

    async fn get(
        &self,
        id: String,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError>;
}

pub struct ApiSecuritySchemeServiceLive<ProjectContext> {
    pub client: Box<dyn ApiSecurityClient<ProjectContext = ProjectContext> + Send + Sync>,
}

#[async_trait]
impl<ProjectContext: Send + Sync> ApiSecuritySchemeService
    for ApiSecuritySchemeServiceLive<ProjectContext>
{
    type ProjectContext = ProjectContext;

    async fn create(
        &self,
        id: String,
        provider_type: Provider,
        client_id: String,
        client_secret: String,
        scope: Vec<String>,
        redirect_url: String,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError> {
        let deployment = self
            .client
            .create(
                id,
                provider_type,
                client_id,
                client_secret,
                scope,
                redirect_url,
                project,
            )
            .await?;

        Ok(GolemResult::Ok(Box::new(deployment)))
    }

    async fn get(
        &self,
        id: String,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError> {
        let deployment = self.client.get(&id, project).await?;

        Ok(GolemResult::Ok(Box::new(deployment)))
    }
}
