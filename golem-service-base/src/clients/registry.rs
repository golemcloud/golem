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

use super::RegistryServiceConfig;
use crate::custom_api::CompiledRoutes;
use crate::model::auth::{AuthCtx, AuthDetailsForEnvironment, UserAuthCtx};
use crate::model::{AccountResourceLimits, ResourceLimits};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::registry::FuelUsageUpdate;
use golem_api_grpc::proto::golem::registry::v1::registry_service_client::RegistryServiceClient;
use golem_api_grpc::proto::golem::registry::v1::{
    AuthenticateTokenRequest, BatchUpdateFuelUsageRequest, DownloadComponentRequest,
    GetActiveRoutesForDomainRequest, GetAgentTypeRequest, GetAllAgentTypesRequest,
    GetAllComponentVersionsRequest, GetAuthDetailsForEnvironmentRequest,
    GetComponentMetadataRequest, GetLatestComponentMetadataRequest, GetResourceLimitsRequest,
    ResolveComponentRequest, UpdateWorkerConnectionLimitRequest, UpdateWorkerLimitRequest,
    authenticate_token_response, batch_update_fuel_usage_response, download_component_response,
    get_active_routes_for_domain_response, get_agent_type_response, get_all_agent_types_response,
    get_all_component_versions_response, get_auth_details_for_environment_response,
    get_component_metadata_response, get_latest_component_metadata_response,
    get_resource_limits_response, resolve_component_response,
    update_worker_connection_limit_response, update_worker_limit_response,
};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::WorkerId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::RegisteredAgentType;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::ComponentDto;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::{IntoAnyhow, SafeDisplay};
use std::collections::HashMap;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;

#[async_trait]
// mirrors golem-api-grpc/proto/golem/registry/v1/registry_service.proto
pub trait RegistryService: Send + Sync {
    // auth api
    async fn authenticate_token(
        &self,
        token: &TokenSecret,
    ) -> Result<AuthCtx, RegistryServiceError>;
    async fn get_auth_details_for_environment(
        &self,
        environment_id: &EnvironmentId,
        include_deleted: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, RegistryServiceError>;

    // limits api
    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth_ctx: &AuthCtx,
    ) -> Result<ResourceLimits, RegistryServiceError>;

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        added: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RegistryServiceError>;

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        added: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RegistryServiceError>;

    // will be a noop if the account no longer exists
    // will return all current limits of updated accounts
    async fn batch_update_fuel_usage(
        &self,
        updates: HashMap<AccountId, i64>,
        auth_ctx: &AuthCtx,
    ) -> Result<AccountResourceLimits, RegistryServiceError>;

    // components api
    // will return the component even if it is deleted
    async fn download_component(
        &self,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, RegistryServiceError>;

    // will also return metadata for deleted components
    async fn get_component_metadata(
        &self,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, RegistryServiceError>;

    // will only return non-deleted components
    async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, RegistryServiceError>;

    // will only return non-deleted components
    async fn get_all_component_versions(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<ComponentDto>, RegistryServiceError>;

    // will only return non-deleted components
    async fn resolve_component(
        &self,
        resolving_account_id: &AccountId,
        resolving_application_id: &ApplicationId,
        resolving_environment_id: &EnvironmentId,
        component_slug: &str,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, RegistryServiceError>;

    // agent types api
    async fn get_all_agent_types(
        &self,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError>;

    async fn get_agent_type(
        &self,
        environment_id: &EnvironmentId,
        name: &str,
    ) -> Result<RegisteredAgentType, RegistryServiceError>;

    async fn get_active_routes_for_domain(
        &self,
        domain: &Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError>;
}

#[derive(Clone)]
pub struct GrpcRegistryService {
    client: GrpcClient<RegistryServiceClient<OtelGrpcService<Channel>>>,
}

impl GrpcRegistryService {
    pub fn new(config: &RegistryServiceConfig) -> Self {
        let max_message_size = config.max_message_size;
        let client = GrpcClient::new(
            "registry",
            move |channel| {
                RegistryServiceClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(max_message_size)
            },
            config.uri(),
            GrpcClientConfig {
                retries_on_unavailable: config.retries.clone(),
                connect_timeout: config.connect_timeout,
            },
        );
        Self { client }
    }
}

#[async_trait]
impl RegistryService for GrpcRegistryService {
    async fn authenticate_token(
        &self,
        token: &TokenSecret,
    ) -> Result<AuthCtx, RegistryServiceError> {
        let response = self
            .client
            .call("authenticate-token", move |client| {
                let request = AuthenticateTokenRequest {
                    secret: token.secret().to_string(),
                };
                Box::pin(client.authenticate_token(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(authenticate_token_response::Result::Success(payload)) => {
                let user_auth_ctx: UserAuthCtx = payload
                    .auth_ctx
                    .ok_or("missing authctx field")?
                    .try_into()?;
                Ok(AuthCtx::User(user_auth_ctx))
            }
            Some(authenticate_token_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn get_auth_details_for_environment(
        &self,
        environment_id: &EnvironmentId,
        include_deleted: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
        let response = self
            .client
            .call("get_auth_details_for_environment", move |client| {
                let request = GetAuthDetailsForEnvironmentRequest {
                    environment_id: Some((*environment_id).into()),
                    include_deleted,
                    auth_ctx: Some(auth_ctx.clone().into()),
                };
                Box::pin(client.get_auth_details_for_environment(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_auth_details_for_environment_response::Result::Success(payload)) => {
                let auth_details: AuthDetailsForEnvironment = payload
                    .auth_details_for_environment
                    .ok_or("missing auth_details_for_environment field")?
                    .try_into()?;
                Ok(auth_details)
            }
            Some(get_auth_details_for_environment_response::Result::Error(error)) => {
                Err(error.into())
            }
        }
    }

    async fn get_resource_limits(
        &self,
        account_id: &AccountId,
        auth_ctx: &AuthCtx,
    ) -> Result<ResourceLimits, RegistryServiceError> {
        let response = self
            .client
            .call("get-resource-limits", move |client| {
                let request = GetResourceLimitsRequest {
                    account_id: Some((*account_id).into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                };
                Box::pin(client.get_resource_limits(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_resource_limits_response::Result::Success(payload)) => {
                Ok(payload.limits.ok_or("missing limits field")?.into())
            }
            Some(get_resource_limits_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn update_worker_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        added: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RegistryServiceError> {
        let response = self
            .client
            .call("update-worker-limit", move |client| {
                let request = UpdateWorkerLimitRequest {
                    account_id: Some((*account_id).into()),
                    worker_id: Some(worker_id.clone().into()),
                    added,
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.update_worker_limit(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(update_worker_limit_response::Result::Success(_)) => Ok(()),
            Some(update_worker_limit_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn update_worker_connection_limit(
        &self,
        account_id: &AccountId,
        worker_id: &WorkerId,
        added: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<(), RegistryServiceError> {
        let response = self
            .client
            .call("update-worker-connection-limit", move |client| {
                let request = UpdateWorkerConnectionLimitRequest {
                    account_id: Some((*account_id).into()),
                    worker_id: Some(worker_id.clone().into()),
                    added,
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.update_worker_connection_limit(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(update_worker_connection_limit_response::Result::Success(_)) => Ok(()),
            Some(update_worker_connection_limit_response::Result::Error(error)) => {
                Err(error.into())
            }
        }
    }

    async fn batch_update_fuel_usage(
        &self,
        updates: HashMap<AccountId, i64>,
        auth_ctx: &AuthCtx,
    ) -> Result<AccountResourceLimits, RegistryServiceError> {
        let updates: Vec<FuelUsageUpdate> = updates
            .into_iter()
            .map(|(k, v)| FuelUsageUpdate {
                account_id: Some(k.into()),
                value: v,
            })
            .collect();

        let response = self
            .client
            .call("batch-update-fuel-usage", move |client| {
                let request = BatchUpdateFuelUsageRequest {
                    updates: updates.clone(),
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.batch_update_fuel_usage(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(batch_update_fuel_usage_response::Result::Success(payload)) => {
                let converted = payload
                    .account_resource_limits
                    .ok_or("missing account_resource_limits field")?
                    .try_into()?;
                Ok(converted)
            }
            Some(batch_update_fuel_usage_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn download_component(
        &self,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        let mut response = self
            .client
            .call("download-component", move |client| {
                let request = DownloadComponentRequest {
                    component_id: Some((*component_id).into()),
                    version: component_revision.0,
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.download_component(request))
            })
            .await?
            .into_inner();

        let mut bytes = Vec::new();

        while let Some(message) = response.message().await? {
            match message.result {
                None => return Err(RegistryServiceError::empty_response()),
                Some(download_component_response::Result::SuccessChunk(chunk)) => {
                    bytes.extend_from_slice(&chunk)
                }
                Some(download_component_response::Result::Error(error)) => Err(error)?,
            };
        }

        Ok(bytes)
    }

    async fn get_component_metadata(
        &self,
        component_id: &ComponentId,
        component_revision: ComponentRevision,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, RegistryServiceError> {
        let response = self
            .client
            .call("get-component-metadata", move |client| {
                let request = GetComponentMetadataRequest {
                    component_id: Some((*component_id).into()),
                    version: component_revision.0,
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.get_component_metadata(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_component_metadata_response::Result::Success(payload)) => {
                let converted = payload
                    .component
                    .ok_or("missing component field")?
                    .try_into()?;
                Ok(converted)
            }
            Some(get_component_metadata_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn get_latest_component_metadata(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, RegistryServiceError> {
        let response = self
            .client
            .call("get-latest-component-metadata", move |client| {
                let request = GetLatestComponentMetadataRequest {
                    component_id: Some((*component_id).into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.get_latest_component_metadata(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_latest_component_metadata_response::Result::Success(payload)) => {
                let converted = payload
                    .component
                    .ok_or("missing component field")?
                    .try_into()?;
                Ok(converted)
            }
            Some(get_latest_component_metadata_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn get_all_component_versions(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AuthCtx,
    ) -> Result<Vec<ComponentDto>, RegistryServiceError> {
        let response = self
            .client
            .call("resolve-component-by-name", move |client| {
                let request = GetAllComponentVersionsRequest {
                    component_id: Some((*component_id).into()),
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.get_all_component_versions(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_all_component_versions_response::Result::Success(payload)) => {
                let converted = payload
                    .components
                    .into_iter()
                    .map(ComponentDto::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(converted)
            }
            Some(get_all_component_versions_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn resolve_component(
        &self,
        resolving_account_id: &AccountId,
        resolving_application_id: &ApplicationId,
        resolving_environment_id: &EnvironmentId,
        component_slug: &str,
        auth_ctx: &AuthCtx,
    ) -> Result<ComponentDto, RegistryServiceError> {
        let response = self
            .client
            .call("resolve-component", move |client| {
                let request = ResolveComponentRequest {
                    resolving_account_id: Some((*resolving_account_id).into()),
                    resolving_application_id: Some((*resolving_application_id).into()),
                    resolving_environment_id: Some((*resolving_environment_id).into()),
                    component_slug: component_slug.to_string(),
                    auth_ctx: Some(auth_ctx.clone().into()),
                };

                Box::pin(client.resolve_component(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(resolve_component_response::Result::Success(payload)) => {
                let converted = payload
                    .component
                    .ok_or("missing component field")?
                    .try_into()?;
                Ok(converted)
            }
            Some(resolve_component_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn get_all_agent_types(
        &self,
        environment_id: &EnvironmentId,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
        let response = self
            .client
            .call("get-all-agent-types", move |client| {
                let request = GetAllAgentTypesRequest {
                    environment_id: Some((*environment_id).into()),
                };

                Box::pin(client.get_all_agent_types(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_all_agent_types_response::Result::Success(payload)) => {
                let converted = payload
                    .agent_types
                    .into_iter()
                    .map(RegisteredAgentType::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(converted)
            }
            Some(get_all_agent_types_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn get_agent_type(
        &self,
        environment_id: &EnvironmentId,
        name: &str,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        let response = self
            .client
            .call("get-all-agent-types", move |client| {
                let request = GetAgentTypeRequest {
                    environment_id: Some((*environment_id).into()),
                    agent_type: name.to_string(),
                };
                Box::pin(client.get_agent_type(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_agent_type_response::Result::Success(payload)) => {
                let converted = payload
                    .agent_type
                    .ok_or("missing agent_type field")?
                    .try_into()?;
                Ok(converted)
            }
            Some(get_agent_type_response::Result::Error(error)) => Err(error.into()),
        }
    }

    async fn get_active_routes_for_domain(
        &self,
        domain: &Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError> {
        let response = self
            .client
            .call("get-active-routes-for-domain", move |client| {
                let request = GetActiveRoutesForDomainRequest {
                    domain: domain.0.clone(),
                };
                Box::pin(client.get_active_routes_for_domain(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_active_routes_for_domain_response::Result::Success(payload)) => {
                let converted = payload
                    .compiled_routes
                    .ok_or("missing compiled_routes field")?
                    .try_into()?;
                Ok(converted)
            }
            Some(get_active_routes_for_domain_response::Result::Error(error)) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum RegistryServiceError {
    #[error("BadRequest: {0:?}")]
    BadRequest(Vec<String>),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    LimitExceeded(String),
    #[error("NotFound: {0}")]
    NotFound(String),
    #[error("AlreadyExists: {0}")]
    AlreadyExists(String),
    #[error("Internal error: {0}")]
    InternalError(String),
    #[error("Cound not authenticate: {0}")]
    CouldNotAuthenticate(String),
    #[error("Internal client error: {0}")]
    InternalClientError(String),
}

impl RegistryServiceError {
    pub fn internal_client_error(error: impl AsRef<str>) -> Self {
        Self::InternalClientError(error.as_ref().to_string())
    }

    pub fn empty_response() -> Self {
        Self::internal_client_error("empty response")
    }
}

impl SafeDisplay for RegistryServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AlreadyExists(_) => self.to_string(),
            Self::BadRequest(_) => self.to_string(),
            Self::CouldNotAuthenticate(_) => self.to_string(),
            Self::LimitExceeded(_) => self.to_string(),
            Self::NotFound(_) => self.to_string(),
            Self::Unauthorized(_) => self.to_string(),
            Self::InternalClientError(_) => "Internal error".to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

impl IntoAnyhow for RegistryServiceError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::from(self).context("RegistryServiceError")
    }
}

impl From<golem_api_grpc::proto::golem::registry::v1::RegistryServiceError>
    for RegistryServiceError
{
    fn from(value: golem_api_grpc::proto::golem::registry::v1::RegistryServiceError) -> Self {
        use golem_api_grpc::proto::golem::registry::v1::registry_service_error::Error;
        match value.error {
            Some(Error::LimitExceeded(error)) => Self::LimitExceeded(error.error),
            Some(Error::NotFound(error)) => Self::NotFound(error.error),
            Some(Error::AlreadyExists(error)) => Self::AlreadyExists(error.error),
            Some(Error::BadRequest(errors)) => Self::BadRequest(errors.errors),
            Some(Error::InternalError(error)) => Self::InternalError(error.error),
            Some(Error::Unauthorized(error)) => Self::Unauthorized(error.error),
            Some(Error::CouldNotAuthenticate(error)) => Self::CouldNotAuthenticate(error.error),
            None => Self::internal_client_error("Missing error field"),
        }
    }
}

impl From<tonic::transport::Error> for RegistryServiceError {
    fn from(error: tonic::transport::Error) -> Self {
        Self::internal_client_error(format!("Transport error: {error}"))
    }
}

impl From<tonic::Status> for RegistryServiceError {
    fn from(status: tonic::Status) -> Self {
        Self::internal_client_error(format!("Conection error: {status}"))
    }
}

impl From<String> for RegistryServiceError {
    fn from(value: String) -> Self {
        Self::internal_client_error(value)
    }
}

impl From<&'static str> for RegistryServiceError {
    fn from(value: &'static str) -> Self {
        Self::internal_client_error(value)
    }
}
