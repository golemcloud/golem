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
use golem_cloud_client::model::{ProjectActions, ProjectPolicy, ProjectPolicyData};
use tracing::info;

use crate::clients::action_cli_to_api;
use crate::model::{GolemError, ProjectAction, ProjectPolicyId};

#[async_trait]
pub trait ProjectPolicyClient {
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectPolicy, GolemError>;
    async fn get(&self, policy_id: ProjectPolicyId) -> Result<ProjectPolicy, GolemError>;
}

pub struct ProjectPolicyClientLive<C: golem_cloud_client::api::ProjectPolicyClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ProjectPolicyClient + Sync + Send> ProjectPolicyClient
    for ProjectPolicyClientLive<C>
{
    async fn create(
        &self,
        name: String,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectPolicy, GolemError> {
        info!("Creation project policy");

        let actions: Vec<golem_cloud_client::model::ProjectAction> =
            actions.into_iter().map(action_cli_to_api).collect();
        let data = ProjectPolicyData {
            name,
            project_actions: ProjectActions { actions },
        };

        Ok(self.client.post(&data).await?)
    }

    async fn get(&self, policy_id: ProjectPolicyId) -> Result<ProjectPolicy, GolemError> {
        info!("Getting project policy");

        Ok(self.client.project_policy_id_get(&policy_id.0).await?)
    }
}
