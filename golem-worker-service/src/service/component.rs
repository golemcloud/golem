// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_service_base::model::auth::{AuthCtx, UserAuthCtx};
use async_trait::async_trait;
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::RetryConfig;
use golem_common::model::auth::TokenSecret;
use golem_common::retries::with_retries;
use std::fmt::Display;
use tonic::Status;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use golem_api_grpc::proto::golem::registry::v1::registry_service_client::RegistryServiceClient;
use golem_api_grpc::proto::golem::registry::v1::{authenticate_token_response, download_component_response, get_agent_type_response, get_all_agent_types_response, get_component_metadata_response, get_components_response, get_plugin_registration_by_id_response, get_resource_limits_response, update_worker_limit_response, AuthenticateTokenRequest, DownloadComponentRequest, GetAgentTypeRequest, GetAllAgentTypesRequest, GetComponentMetadataRequest, GetComponentsRequest, GetLatestComponentRequest, GetPluginRegistrationByIdRequest, GetResourceLimitsRequest, UpdateWorkerLimitRequest};
use golem_service_base::model::ResourceLimits;
use golem_common::model::WorkerId;
use golem_common::model::account::AccountId;
use tracing::info;
use golem_common::model::plugin_registration::{PluginRegistrationId};
use golem_service_base::model::plugin_registration::PluginRegistration;
use golem_common::model::component::ComponentDto;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::agent::RegisteredAgentType;
use golem_common::{error_forwarding, SafeDisplay};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use std::sync::Arc;
use golem_common::model::component::ComponentName;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::cache::SimpleCache;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ComponentServiceError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
    #[error("Component not found")]
    ComponentNotFound,
}

impl SafeDisplay for ComponentServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::ComponentNotFound => "Component not found".to_string()
        }
    }
}

error_forwarding!(ComponentServiceError);

#[async_trait]
pub trait ComponentService: Send + Sync {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError>;

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError>;
}

pub struct CachedComponentService {
    inner: Arc<dyn ComponentService>,
    cache: Cache<(ComponentId, ComponentRevision), (), ComponentDto, ComponentServiceError>,
}

impl CachedComponentService {
    pub fn new(inner: Arc<dyn ComponentService>, cache_capacity: usize) -> Self {
        Self {
            inner,
            cache: Cache::new(
                Some(cache_capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "component-metadata-cache",
            ),
        }
    }
}

#[async_trait]
impl ComponentService for CachedComponentService {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        let inner_clone = self.inner.clone();
        self.cache
            .get_or_insert_simple(&(component_id.clone(), version), || async {
                inner_clone
                    .get_by_version(component_id, version, auth_ctx)
                    .await
            })
            .await
    }

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        self.inner.get_latest_by_id(component_id, auth_ctx).await
    }
}

pub struct RemoteComponentService {
    client: Arc<dyn RegistryService>
}

impl RemoteComponentService {
    pub fn new(client: Arc<dyn RegistryService>) -> Self {
        Self {
            client
        }
    }
}

#[async_trait]
impl ComponentService for RemoteComponentService {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        self.client.get_component_metadata(component_id, version, auth_ctx).await.map_err(|e| match e {
            RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
            other => other.into()
        })
    }

    async fn get_latest_by_id(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, ComponentServiceError> {
        self.client.get_latest_component_metadata(component_id, auth_ctx).await.map_err(|e| match e {
            RegistryServiceError::NotFound(_) => ComponentServiceError::ComponentNotFound,
            other => other.into()
        })
    }
}
