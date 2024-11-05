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

use crate::model::{
    ApiDefinitionFileFormat, ApiDefinitionId, ApiDefinitionVersion, GolemError, PathBufOrStdin,
};
use async_trait::async_trait;
use golem_client::model::HttpApiDefinitionWithTypeInfo;

#[async_trait]
pub trait ApiDefinitionClient {
    type ProjectContext;

    async fn list(
        &self,
        id: Option<&ApiDefinitionId>,
        project: &Self::ProjectContext,
    ) -> Result<Vec<HttpApiDefinitionWithTypeInfo>, GolemError>;
    async fn get(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<HttpApiDefinitionWithTypeInfo, GolemError>;
    async fn create(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<HttpApiDefinitionWithTypeInfo, GolemError>;
    async fn update(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<HttpApiDefinitionWithTypeInfo, GolemError>;
    async fn import(
        &self,
        path: PathBufOrStdin,
        project: &Self::ProjectContext,
        format: &ApiDefinitionFileFormat,
    ) -> Result<HttpApiDefinitionWithTypeInfo, GolemError>;
    async fn delete(
        &self,
        id: ApiDefinitionId,
        version: ApiDefinitionVersion,
        project: &Self::ProjectContext,
    ) -> Result<String, GolemError>;
}
