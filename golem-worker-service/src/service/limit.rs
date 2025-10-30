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

#[derive(Debug, thiserror::Error)]
pub enum LimitServiceError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
    #[error("Limit exceeded: {0}")]
    LimitExceeded(String),
}

impl SafeDisplay for LimitServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::LimitExceeded(_) => self.to_string()
        }
    }
}

error_forwarding!(LimitServiceError, RegistryServiceError);

#[async_trait]
pub trait LimitService: Send + Sync {
    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<ResourceLimits, LimitServiceError>;

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitServiceError>;

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitServiceError>;
}

pub struct RemoteLimitService {
    client: Arc<dyn RegistryService>
}

impl RemoteLimitService {
    pub fn new(client: Arc<dyn RegistryService>) -> Self {
        Self {
            client
        }
    }
}

#[async_trait]
impl LimitService for RemoteLimitService {
    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
    ) -> Result<ResourceLimits, LimitServiceError> {
        Ok(self.client.get_resource_limits(account_id, &AuthCtx::System).await?)
    }

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitServiceError> {
        self.client.update_worker_limit(account_id, worker_id, value, &AuthCtx::System).await.map_err(|e| match e {
            RegistryServiceError::LimitExceeded(msg) => LimitServiceError::LimitExceeded(msg),
            other => other.into()
        })?;
        Ok(())
    }

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        value: i32,
    ) -> Result<(), LimitServiceError>  {
        self.client.update_worker_connection_limit(account_id, worker_id, value, &AuthCtx::System).await.map_err(|e| match e {
            RegistryServiceError::LimitExceeded(msg) => LimitServiceError::LimitExceeded(msg),
            other => other.into()
        })?;
        Ok(())
    }
}
