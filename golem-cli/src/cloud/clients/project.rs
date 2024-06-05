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

use crate::cloud::model::{AccountId, ProjectId};
use async_trait::async_trait;
use golem_cloud_client::model::{Project, ProjectDataRequest};
use tracing::info;

use crate::model::GolemError;

#[async_trait]
pub trait ProjectClient {
    async fn create(
        &self,
        owner_account_id: &AccountId,
        name: String,
        description: Option<String>,
    ) -> Result<Project, GolemError>;
    async fn find(&self, name: Option<String>) -> Result<Vec<Project>, GolemError>;
    async fn find_default(&self) -> Result<Project, GolemError>;
    async fn delete(&self, project_id: ProjectId) -> Result<(), GolemError>;
}

pub struct ProjectClientLive<C: golem_cloud_client::api::ProjectClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ProjectClient + Sync + Send> ProjectClient
    for ProjectClientLive<C>
{
    async fn create(
        &self,
        owner_account_id: &AccountId,
        name: String,
        description: Option<String>,
    ) -> Result<Project, GolemError> {
        info!("Create new project {name}.");

        let request = ProjectDataRequest {
            name,
            owner_account_id: owner_account_id.id.to_string(),
            description: description.unwrap_or("".to_string()),
        };
        Ok(self.client.post(&request).await?)
    }

    async fn find(&self, name: Option<String>) -> Result<Vec<Project>, GolemError> {
        info!("Listing projects.");

        Ok(self.client.get(name.as_deref()).await?)
    }

    async fn find_default(&self) -> Result<Project, GolemError> {
        info!("Getting default project.");

        Ok(self.client.default_get().await?)
    }

    async fn delete(&self, project_id: ProjectId) -> Result<(), GolemError> {
        info!("Deleting project {project_id:?}");

        let _ = self.client.project_id_delete(&project_id.0).await?;

        Ok(())
    }
}
