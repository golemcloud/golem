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
use golem_gateway_client::model::{ApiDomain, DomainRequest};

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait DomainClient {
    async fn get(&self, project_id: ProjectId) -> Result<Vec<ApiDomain>, GolemError>;

    async fn update(
        &self,
        project_id: ProjectId,
        domain_name: String,
    ) -> Result<ApiDomain, GolemError>;

    async fn delete(&self, project_id: ProjectId, domain_name: &str) -> Result<String, GolemError>;
}

pub struct DomainClientLive<C: golem_gateway_client::api::ApiDomainClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::ApiDomainClient + Sync + Send> DomainClient
    for DomainClientLive<C>
{
    async fn get(&self, project_id: ProjectId) -> Result<Vec<ApiDomain>, GolemError> {
        Ok(self.client.get(&project_id.0).await?)
    }

    async fn update(
        &self,
        project_id: ProjectId,
        domain_name: String,
    ) -> Result<ApiDomain, GolemError> {
        Ok(self
            .client
            .put(&DomainRequest {
                project_id: project_id.0,
                domain_name,
            })
            .await?)
    }

    async fn delete(&self, project_id: ProjectId, domain_name: &str) -> Result<String, GolemError> {
        Ok(self.client.delete(&project_id.0, domain_name).await?)
    }
}
