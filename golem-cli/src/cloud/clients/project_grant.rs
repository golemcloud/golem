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
use golem_cloud_client::model::{ProjectGrant, ProjectGrantDataRequest};
use tracing::info;

use crate::cloud::clients::action_cli_to_api;
use crate::cloud::model::{AccountId, ProjectAction, ProjectId, ProjectPolicyId};
use crate::model::GolemError;

#[async_trait]
pub trait ProjectGrantClient {
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
    ) -> Result<ProjectGrant, GolemError>;
    async fn create_actions(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectGrant, GolemError>;
}

pub struct ProjectGrantClientLive<C: golem_cloud_client::api::ProjectGrantClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::ProjectGrantClient + Sync + Send> ProjectGrantClient
    for ProjectGrantClientLive<C>
{
    async fn create(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        policy_id: ProjectPolicyId,
    ) -> Result<ProjectGrant, GolemError> {
        info!("Creating project grant for policy {policy_id}.");

        let data = ProjectGrantDataRequest {
            grantee_account_id: account_id.id,
            project_policy_id: Some(policy_id.0),
            project_actions: Vec::new(),
            project_policy_name: None,
        };

        Ok(self.client.post(&project_id.0, &data).await?)
    }

    async fn create_actions(
        &self,
        project_id: ProjectId,
        account_id: AccountId,
        actions: Vec<ProjectAction>,
    ) -> Result<ProjectGrant, GolemError> {
        info!("Creating project grant for actions.");

        let data = ProjectGrantDataRequest {
            grantee_account_id: account_id.id,
            project_policy_id: None,
            project_policy_name: None,
            project_actions: actions.into_iter().map(action_cli_to_api).collect(),
        };

        Ok(self.client.post(&project_id.0, &data).await?)
    }
}
