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
use golem_gateway_client::model::HttpApiDefinition;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait DefinitionClient {
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: Option<&str>,
    ) -> Result<Vec<HttpApiDefinition>, GolemError>;

    async fn update(
        &self,
        project_id: ProjectId,
        api_definition: HttpApiDefinition,
    ) -> Result<HttpApiDefinition, GolemError>;

    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
        version: &str,
    ) -> Result<String, GolemError>;
}

pub struct DefinitionClientLive<C: golem_gateway_client::api::ApiDefinitionClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::ApiDefinitionClient + Sync + Send> DefinitionClient
    for DefinitionClientLive<C>
{
    async fn get(
        &self,
        project_id: ProjectId,
        api_definition_id: Option<&str>,
    ) -> Result<Vec<HttpApiDefinition>, GolemError> {
        Ok(self.client.get(&project_id.0, api_definition_id).await?)
    }

    async fn update(
        &self,
        project_id: ProjectId,
        api_definition: HttpApiDefinition,
    ) -> Result<HttpApiDefinition, GolemError> {
        Ok(self.client.put(&project_id.0, &api_definition).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        api_definition_id: &str,
        version: &str,
    ) -> Result<String, GolemError> {
        Ok(self
            .client
            .delete(&project_id.0, api_definition_id, version)
            .await?)
    }
}
