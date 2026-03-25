// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::error::SharedError;
use golem_common::model::resource_definition::{
    ResourceDefinition, ResourceDefinitionId, ResourceName,
};
use golem_common::{IntoAnyhow, SafeDisplay};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use std::sync::Arc;

/// Clone is required because this is used as the error type in `golem_common::cache::Cache`.
#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("Resource definition not found")]
    NotFound,
    #[error(transparent)]
    InternalError(SharedError),
}

impl IntoAnyhow for FetchError {
    fn into_anyhow(self) -> ::anyhow::Error {
        anyhow::Error::from(self).context("FetchError")
    }
}

impl SafeDisplay for FetchError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::NotFound => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

impl From<RegistryServiceError> for FetchError {
    fn from(err: RegistryServiceError) -> Self {
        match err {
            RegistryServiceError::NotFound(_) => FetchError::NotFound,
            other => FetchError::InternalError(SharedError::new(other)),
        }
    }
}

#[async_trait]
pub trait ResourceDefinitionFetcher: Send + Sync {
    async fn get_by_id(&self, id: ResourceDefinitionId) -> Result<ResourceDefinition, FetchError>;

    async fn get_by_name(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
    ) -> Result<ResourceDefinition, FetchError>;
}

pub struct GrpcResourceDefinitionFetcher {
    registry_service: Arc<dyn RegistryService>,
}

impl GrpcResourceDefinitionFetcher {
    pub fn new(registry_service: Arc<dyn RegistryService>) -> Self {
        Self { registry_service }
    }
}

#[async_trait]
impl ResourceDefinitionFetcher for GrpcResourceDefinitionFetcher {
    async fn get_by_id(&self, id: ResourceDefinitionId) -> Result<ResourceDefinition, FetchError> {
        self.registry_service
            .get_resource_definition_by_id(id)
            .await
            .map_err(FetchError::from)
    }

    async fn get_by_name(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
    ) -> Result<ResourceDefinition, FetchError> {
        self.registry_service
            .get_resource_definition_by_name(environment_id, name)
            .await
            .map_err(FetchError::from)
    }
}
