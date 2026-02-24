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

use crate::custom_api::{CompiledRoutes};
use crate::grpc::client::{GrpcClient, GrpcClientConfig};
use crate::model::auth::{AuthCtx, AuthDetailsForEnvironment, UserAuthCtx};
use crate::model::{AccountResourceLimits, AgentDeploymentDetails, ResourceLimits};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::registry::FuelUsageUpdate;
use golem_api_grpc::proto::golem::registry::v1::registry_service_client::RegistryServiceClient;
use golem_api_grpc::proto::golem::registry::v1::{
    AuthenticateTokenRequest, BatchUpdateFuelUsageRequest, DownloadComponentRequest,
    GetActiveRoutesForDomainRequest, GetAgentDeploymentsRequest, GetAgentTypeRequest,
    GetAllAgentTypesRequest, GetAllDeployedComponentRevisionsRequest,
    GetAuthDetailsForEnvironmentRequest, GetComponentMetadataRequest,
    GetDeployedComponentMetadataRequest, GetResourceLimitsRequest, ResolveComponentRequest,
    UpdateWorkerConnectionLimitRequest, UpdateWorkerLimitRequest, authenticate_token_response,
    batch_update_fuel_usage_response, download_component_response,
    get_active_routes_for_domain_response, get_agent_deployments_response, get_agent_type_response,
    get_all_agent_types_response, get_all_deployed_component_revisions_response,
    get_auth_details_for_environment_response, get_component_metadata_response,
    get_deployed_component_metadata_response, get_resource_limits_response,
    resolve_component_response, resolve_latest_agent_type_by_names_response,
    update_worker_connection_limit_response, update_worker_limit_response,
};
use golem_common::config::{ConfigExample, HasConfigExamples};
use golem_common::model::WorkerId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::ComponentDto;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::{IntoAnyhow, SafeDisplay, grpc_uri};
use http::Uri;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tonic_tracing_opentelemetry::middleware::client::OtelGrpcService;
use crate::mcp::CompiledMcp;

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
        environment_id: EnvironmentId,
        include_deleted: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, RegistryServiceError>;

    // limits api
    async fn get_resource_limits(
        &self,
        account_id: AccountId,
    ) -> Result<ResourceLimits, RegistryServiceError>;

    async fn update_worker_limit(
        &self,
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), RegistryServiceError>;

    async fn update_worker_connection_limit(
        &self,
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), RegistryServiceError>;

    // will be a noop if the account no longer exists
    // will return all current limits of updated accounts
    async fn batch_update_fuel_usage(
        &self,
        updates: HashMap<AccountId, i64>,
    ) -> Result<AccountResourceLimits, RegistryServiceError>;

    // components api
    // will return the component even if it is deleted
    async fn download_component(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError>;

    // will also return metadata for deleted components
    async fn get_component_metadata(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<ComponentDto, RegistryServiceError>;

    // will only return non-deleted components
    async fn get_deployed_component_metadata(
        &self,
        component_id: ComponentId,
    ) -> Result<ComponentDto, RegistryServiceError>;

    // will only return non-deleted components
    async fn get_all_deployed_component_revisions(
        &self,
        component_id: ComponentId,
    ) -> Result<Vec<ComponentDto>, RegistryServiceError>;

    // will only return non-deleted components
    async fn resolve_component(
        &self,
        resolving_account_id: AccountId,
        resolving_application_id: ApplicationId,
        resolving_environment_id: EnvironmentId,
        component_slug: &str,
    ) -> Result<ComponentDto, RegistryServiceError>;

    // agent types api
    async fn get_all_agent_types(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError>;

    async fn get_agent_type(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        name: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError>;

    async fn resolve_latest_agent_type_by_names(
        &self,
        account_id: &AccountId,
        app_name: &ApplicationName,
        environment_name: &EnvironmentName,
        agent_type_name: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError>;

    async fn get_active_routes_for_domain(
        &self,
        domain: &Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError>;

    async fn get_active_mcp_capabilities_for_domain(
        &self,
        domain: &Domain,
    ) -> Result<CompiledMcp, RegistryServiceError>;

    async fn get_agent_deployments(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<HashMap<AgentTypeName, AgentDeploymentDetails>, RegistryServiceError>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GrpcRegistryServiceConfig {
    pub host: String,
    pub port: u16,
    pub max_message_size: usize,
    #[serde(flatten)]
    pub client_config: GrpcClientConfig,
}

impl GrpcRegistryServiceConfig {
    pub fn uri(&self) -> Uri {
        grpc_uri(&self.host, self.port, self.client_config.tls_enabled())
    }
}

impl SafeDisplay for GrpcRegistryServiceConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "max_message_size: {}", self.max_message_size);
        let _ = writeln!(&mut result, "{}", self.client_config.to_safe_string());
        result
    }
}

impl Default for GrpcRegistryServiceConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 8080,
            max_message_size: 50 * 1024 * 1024,
            client_config: GrpcClientConfig::default(),
        }
    }
}

impl HasConfigExamples<GrpcRegistryServiceConfig> for GrpcRegistryServiceConfig {
    fn examples() -> Vec<ConfigExample<GrpcRegistryServiceConfig>> {
        vec![]
    }
}

#[derive(Clone)]
pub struct GrpcRegistryService {
    client: GrpcClient<RegistryServiceClient<OtelGrpcService<Channel>>>,
}

impl GrpcRegistryService {
    pub fn new(config: &GrpcRegistryServiceConfig) -> Self {
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
            config.client_config.clone(),
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
            .call("authenticate_token", move |client| {
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
        environment_id: EnvironmentId,
        include_deleted: bool,
        auth_ctx: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
        let response = self
            .client
            .call("get_auth_details_for_environment", move |client| {
                let request = GetAuthDetailsForEnvironmentRequest {
                    environment_id: Some(environment_id.into()),
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
        account_id: AccountId,
    ) -> Result<ResourceLimits, RegistryServiceError> {
        let response = self
            .client
            .call("get_resource_limits", move |client| {
                let request = GetResourceLimitsRequest {
                    account_id: Some(account_id.into()),
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
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), RegistryServiceError> {
        let response = self
            .client
            .call("update_worker_limit", move |client| {
                let request = UpdateWorkerLimitRequest {
                    account_id: Some(account_id.into()),
                    worker_id: Some(worker_id.clone().into()),
                    added,
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
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), RegistryServiceError> {
        let response = self
            .client
            .call("update_worker_connection_limit", move |client| {
                let request = UpdateWorkerConnectionLimitRequest {
                    account_id: Some(account_id.into()),
                    worker_id: Some(worker_id.clone().into()),
                    added,
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
            .call("batch_update_fuel_usage", move |client| {
                let request = BatchUpdateFuelUsageRequest {
                    updates: updates.clone(),
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
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        let mut response = self
            .client
            .call("download_component", move |client| {
                let request = DownloadComponentRequest {
                    component_id: Some(component_id.into()),
                    revision: component_revision.into(),
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
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<ComponentDto, RegistryServiceError> {
        let response = self
            .client
            .call("get_component_metadata", move |client| {
                let request = GetComponentMetadataRequest {
                    component_id: Some(component_id.into()),
                    revision: component_revision.into(),
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

    async fn get_deployed_component_metadata(
        &self,
        component_id: ComponentId,
    ) -> Result<ComponentDto, RegistryServiceError> {
        let response = self
            .client
            .call("get_deployed_component_metadata", move |client| {
                let request = GetDeployedComponentMetadataRequest {
                    component_id: Some(component_id.into()),
                };

                Box::pin(client.get_deployed_component_metadata(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_deployed_component_metadata_response::Result::Success(payload)) => {
                let converted = payload
                    .component
                    .ok_or("missing component field")?
                    .try_into()?;
                Ok(converted)
            }
            Some(get_deployed_component_metadata_response::Result::Error(error)) => {
                Err(error.into())
            }
        }
    }

    async fn get_all_deployed_component_revisions(
        &self,
        component_id: ComponentId,
    ) -> Result<Vec<ComponentDto>, RegistryServiceError> {
        let response = self
            .client
            .call("get_all_deployed_component_revisions", move |client| {
                let request = GetAllDeployedComponentRevisionsRequest {
                    component_id: Some(component_id.into()),
                };

                Box::pin(client.get_all_deployed_component_revisions(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_all_deployed_component_revisions_response::Result::Success(payload)) => {
                let converted = payload
                    .components
                    .into_iter()
                    .map(ComponentDto::try_from)
                    .collect::<Result<_, _>>()?;
                Ok(converted)
            }
            Some(get_all_deployed_component_revisions_response::Result::Error(error)) => {
                Err(error.into())
            }
        }
    }

    async fn resolve_component(
        &self,
        resolving_account_id: AccountId,
        resolving_application_id: ApplicationId,
        resolving_environment_id: EnvironmentId,
        component_slug: &str,
    ) -> Result<ComponentDto, RegistryServiceError> {
        let response = self
            .client
            .call("resolve_component", move |client| {
                let request = ResolveComponentRequest {
                    resolving_account_id: Some(resolving_account_id.into()),
                    resolving_application_id: Some(resolving_application_id.into()),
                    resolving_environment_id: Some(resolving_environment_id.into()),
                    component_slug: component_slug.to_string(),
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
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
        let response = self
            .client
            .call("get_all_agent_types", move |client| {
                let request = GetAllAgentTypesRequest {
                    environment_id: Some(environment_id.into()),
                    component_id: Some(component_id.into()),
                    component_revision: component_revision.into(),
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
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        name: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        let response = self
            .client
            .call("get_agent_type", move |client| {
                let request = GetAgentTypeRequest {
                    environment_id: Some(environment_id.into()),
                    component_id: Some(component_id.into()),
                    component_revision: component_revision.into(),
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

    async fn resolve_latest_agent_type_by_names(
        &self,
        account_id: &AccountId,
        app_name: &ApplicationName,
        environment_name: &EnvironmentName,
        agent_type_name: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        let response = self
            .client
            .call("resolve_latest_agent_type_by_names", move |client| {
                let request = golem_api_grpc::proto::golem::registry::v1::ResolveLatestAgentTypeByNamesRequest {
                    account_id: Some((*account_id).into()),
                    app_name: app_name.0.clone(),
                    environment_name: environment_name.0.clone(),
                    agent_type_name: agent_type_name.0.clone(),
                };
                Box::pin(client.resolve_latest_agent_type_by_names(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(resolve_latest_agent_type_by_names_response::Result::Success(payload)) => {
                Ok(payload
                    .agent_type
                    .ok_or("missing agent_type field")?
                    .try_into()?)
            }
            Some(resolve_latest_agent_type_by_names_response::Result::Error(error)) => {
                Err(error.into())
            }
        }
    }

    async fn get_active_routes_for_domain(
        &self,
        domain: &Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError> {
        let response = self
            .client
            .call("get_active_routes_for_domain", move |client| {
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

    async fn get_active_mcp_capabilities_for_domain(&self, _domain: &Domain) -> Result<CompiledMcp, RegistryServiceError> {
        todo!()
    }

    async fn get_agent_deployments(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<HashMap<AgentTypeName, AgentDeploymentDetails>, RegistryServiceError> {
        let response = self
            .client
            .call("get_active_domains_for_agent_types", move |client| {
                let request = GetAgentDeploymentsRequest {
                    environment_id: Some(environment_id.into()),
                };
                Box::pin(client.get_agent_deployments(request))
            })
            .await?
            .into_inner();

        match response.result {
            None => Err(RegistryServiceError::empty_response()),
            Some(get_agent_deployments_response::Result::Success(payload)) => {
                let mut result = HashMap::new();
                for entry in payload.agent_deployment_details {
                    let converted = AgentDeploymentDetails::from(entry);
                    result.insert(converted.agent_type_name.clone(), converted);
                }
                Ok(result)
            }
            Some(get_agent_deployments_response::Result::Error(error)) => Err(error.into()),
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
    #[error("Internal server error: {0}")]
    InternalServerError(String),
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
            Self::InternalServerError(_) => "Internal error".to_string(),
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
            Some(Error::InternalError(error)) => Self::InternalServerError(error.error),
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
