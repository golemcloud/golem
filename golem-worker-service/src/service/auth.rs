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
use golem_api_grpc::proto::golem::registry::v1::{authenticate_token_response, download_component_response, get_agent_type_response, get_all_agent_types_response, get_component_metadata_response, get_plugin_registration_by_id_response, get_resource_limits_response, update_worker_limit_response, AuthenticateTokenRequest, DownloadComponentRequest, GetAgentTypeRequest, GetAllAgentTypesRequest, GetComponentMetadataRequest, GetLatestComponentRequest, GetPluginRegistrationByIdRequest, GetResourceLimitsRequest, UpdateWorkerLimitRequest};
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
pub enum AuthServiceError {
    #[error("Could not authenticate user using token")]
    CouldNotAuthenticate,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AuthServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::CouldNotAuthenticate => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AuthServiceError, RegistryServiceError);

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError>;
}

pub struct RemoteAuthService {
    client: Arc<dyn RegistryService>
}

impl RemoteAuthService {
    pub fn new(client: Arc<dyn RegistryService>) -> Self {
        Self {
            client
        }
    }
}

#[async_trait]
impl AuthService for RemoteAuthService {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        self.client.authenticate_token(token).await.map_err(|e| match e {
            RegistryServiceError::CouldNotAuthenticate(_) => AuthServiceError::CouldNotAuthenticate,
            other => other.into()
        })
    }
}
