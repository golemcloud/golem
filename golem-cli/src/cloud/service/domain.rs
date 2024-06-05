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

use crate::cloud::clients::domain::DomainClient;
use crate::cloud::model::ProjectRef;
use crate::cloud::service::project::ProjectService;
use crate::model::{GolemError, GolemResult};
use async_trait::async_trait;

#[async_trait]
pub trait DomainService {
    async fn get(&self, project_ref: ProjectRef) -> Result<GolemResult, GolemError>;
    async fn add(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError>;
}

pub struct DomainServiceLive {
    pub client: Box<dyn DomainClient + Send + Sync>,
    pub projects: Box<dyn ProjectService + Send + Sync>,
}

#[async_trait]
impl DomainService for DomainServiceLive {
    async fn get(&self, project_ref: ProjectRef) -> Result<GolemResult, GolemError> {
        let project_id = self.projects.resolve_id_or_default(project_ref).await?;

        let res = self.client.get(project_id).await?;

        Ok(GolemResult::Ok(Box::new(res)))
    }

    async fn add(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError> {
        let project_id = self.projects.resolve_id_or_default(project_ref).await?;

        let res = self.client.update(project_id, domain_name).await?;

        Ok(GolemResult::Ok(Box::new(res)))
    }

    async fn delete(
        &self,
        project_ref: ProjectRef,
        domain_name: String,
    ) -> Result<GolemResult, GolemError> {
        let project_id = self.projects.resolve_id_or_default(project_ref).await?;
        let res = self.client.delete(project_id, &domain_name).await?;
        Ok(GolemResult::Str(res))
    }
}
