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

use crate::clients::api_definition::ApiDefinitionClient;
use crate::model::text::api_definition::{
    ApiDefinitionAddView, ApiDefinitionGetView, ApiDefinitionImportView, ApiDefinitionUpdateView,
};
use crate::model::{
    ApiDefinitionFileFormat, ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult,
    PathBufOrStdin,
};
use async_trait::async_trait;

#[async_trait]
pub trait ApiDefinitionService {
    type ProjectContext;
    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError>;
    async fn add(
        &self,
        definition: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<GolemResult, GolemError>;
    async fn update(
        &self,
        definition: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<GolemResult, GolemError>;
    async fn import(
        &self,
        definition: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<GolemResult, GolemError>;
    async fn list(
        &self,
        id: Option<ApiDefinitionId>,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError>;
}

pub struct ApiDefinitionServiceLive<ProjectContext> {
    pub client: Box<dyn ApiDefinitionClient<ProjectContext = ProjectContext> + Send + Sync>,
}

#[async_trait]
impl<ProjectContext: Send + Sync> ApiDefinitionService
    for ApiDefinitionServiceLive<ProjectContext>
{
    type ProjectContext = ProjectContext;

    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError> {
        let definition = self.client.get(id, version, project).await?;
        Ok(GolemResult::Ok(Box::new(ApiDefinitionGetView(definition))))
    }

    async fn add(
        &self,
        definition: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<GolemResult, GolemError> {
        let definition = self.client.create(definition, project, format).await?;
        Ok(GolemResult::Ok(Box::new(ApiDefinitionAddView(definition))))
    }

    async fn update(
        &self,
        definition: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<GolemResult, GolemError> {
        let definition = self.client.update(definition, project, format).await?;
        Ok(GolemResult::Ok(Box::new(ApiDefinitionUpdateView(
            definition,
        ))))
    }

    async fn import(
        &self,
        definition: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<GolemResult, GolemError> {
        let definition = self.client.import(definition, project, format).await?;
        Ok(GolemResult::Ok(Box::new(ApiDefinitionImportView(
            definition,
        ))))
    }

    async fn list(
        &self,
        id: Option<ApiDefinitionId>,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError> {
        let definitions = self.client.list(id.as_ref(), project).await?;
        Ok(GolemResult::Ok(Box::new(definitions)))
    }

    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<GolemResult, GolemError> {
        let result = self.client.delete(id, version, project).await?;
        Ok(GolemResult::Str(result))
    }
}
