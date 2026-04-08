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

use super::error::GrpcApiError;
use crate::repo::registry_change::RegistryChangeRepo;
use crate::services::account_usage::{AccountUsageService, ResourceUsageUpdate};
use crate::services::auth::AuthService;
use crate::services::component::ComponentService;
use crate::services::component_resolver::ComponentResolverService;
use crate::services::deployment::{DeployedMcpService, DeployedRoutesService, DeploymentService};
use crate::services::environment::EnvironmentService;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::registry_change_notifier::RegistryChangeNotifier;
use crate::services::resource_definition::ResourceDefinitionService;
use applying::Apply;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use golem_api_grpc::proto::golem::common::Empty as EmptySuccessResponse;
use golem_api_grpc::proto::golem::registry::v1::{
    AuthenticateTokenRequest, AuthenticateTokenResponse, AuthenticateTokenSuccessResponse,
    BatchUpdateResourceUsageRequest, BatchUpdateResourceUsageResponse,
    BatchUpdateResourceUsageSuccessResponse, DownloadComponentRequest, DownloadComponentResponse,
    GetActiveMcpForDomainRequest, GetActiveMcpForDomainResponse,
    GetActiveMcpForDomainSuccessResponse, GetActiveRoutesForDomainRequest,
    GetActiveRoutesForDomainResponse, GetActiveRoutesForDomainSuccessResponse, GetAgentTypeRequest,
    GetAgentTypeResponse, GetAgentTypeSuccessResponse, GetAllAgentTypesRequest,
    GetAllAgentTypesResponse, GetAllAgentTypesSuccessResponse,
    GetAllDeployedComponentRevisionsRequest, GetAllDeployedComponentRevisionsResponse,
    GetAllDeployedComponentRevisionsSuccessResponse, GetAuthDetailsForEnvironmentRequest,
    GetAuthDetailsForEnvironmentResponse, GetAuthDetailsForEnvironmentSuccessResponse,
    GetComponentMetadataRequest, GetComponentMetadataResponse, GetComponentMetadataSuccessResponse,
    GetCurrentEnvironmentStateRequest, GetCurrentEnvironmentStateResponse,
    GetCurrentEnvironmentStateSuccessResponse, GetDeployedComponentMetadataRequest,
    GetDeployedComponentMetadataResponse, GetDeployedComponentMetadataSuccessResponse,
    GetResourceDefinitionByIdRequest, GetResourceDefinitionByIdResponse,
    GetResourceDefinitionByIdSuccessResponse, GetResourceDefinitionByNameRequest,
    GetResourceDefinitionByNameResponse, GetResourceDefinitionByNameSuccessResponse,
    GetResourceLimitsRequest, GetResourceLimitsResponse, GetResourceLimitsSuccessResponse,
    RegistryInvalidationEvent, RegistryServiceError, ResolveAgentTypeByNamesRequest,
    ResolveAgentTypeByNamesResponse, ResolveAgentTypeByNamesSuccessResponse,
    ResolveComponentRequest, ResolveComponentResponse, ResolveComponentSuccessResponse,
    SubscribeRegistryInvalidationsRequest, UpdateWorkerConnectionLimitRequest,
    UpdateWorkerConnectionLimitResponse, UpdateWorkerLimitRequest, UpdateWorkerLimitResponse,
    authenticate_token_response, batch_update_resource_usage_response, download_component_response,
    get_active_mcp_for_domain_response, get_active_routes_for_domain_response,
    get_agent_type_response, get_all_agent_types_response,
    get_all_deployed_component_revisions_response, get_auth_details_for_environment_response,
    get_component_metadata_response, get_current_environment_state_response,
    get_deployed_component_metadata_response, get_resource_definition_by_id_response,
    get_resource_definition_by_name_response, get_resource_limits_response, registry_service_error,
    resolve_agent_type_by_names_response, resolve_component_response,
    update_worker_connection_limit_response, update_worker_limit_response,
};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::resource_definition::{ResourceDefinitionId, ResourceName};
use golem_common::recorded_grpc_api_request;
use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment};
use std::collections::HashMap;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing_futures::Instrument;

pub struct RegistryServiceGrpcApi {
    auth_service: Arc<AuthService>,
    environment_service: Arc<EnvironmentService>,
    account_usage_service: Arc<AccountUsageService>,
    component_service: Arc<ComponentService>,
    component_resolver_service: Arc<ComponentResolverService>,
    deployment_service: Arc<DeploymentService>,
    deployed_routes_service: Arc<DeployedRoutesService>,
    deployed_mcp_service: Arc<DeployedMcpService>,
    environment_state_service: Arc<EnvironmentStateService>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    registry_change_repo: Arc<dyn RegistryChangeRepo>,
    resource_definition_service: Arc<ResourceDefinitionService>,
}

impl RegistryServiceGrpcApi {
    pub fn new(
        auth_service: Arc<AuthService>,
        environment_service: Arc<EnvironmentService>,
        account_usage_service: Arc<AccountUsageService>,
        component_service: Arc<ComponentService>,
        component_resolver_service: Arc<ComponentResolverService>,
        deployment_service: Arc<DeploymentService>,
        deployed_routes_service: Arc<DeployedRoutesService>,
        deployed_mcp_service: Arc<DeployedMcpService>,
        environment_state_service: Arc<EnvironmentStateService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
        registry_change_repo: Arc<dyn RegistryChangeRepo>,
        resource_definition_service: Arc<ResourceDefinitionService>,
    ) -> Self {
        Self {
            auth_service,
            environment_service,
            account_usage_service,
            component_service,
            component_resolver_service,
            deployment_service,
            deployed_routes_service,
            deployed_mcp_service,
            environment_state_service,
            registry_change_notifier,
            registry_change_repo,
            resource_definition_service,
        }
    }

    async fn authenticate_token_internal(
        &self,
        request: AuthenticateTokenRequest,
    ) -> Result<AuthenticateTokenSuccessResponse, GrpcApiError> {
        let auth_ctx = self
            .auth_service
            .authenticate_user(TokenSecret::trusted(request.secret))
            .await?;
        Ok(AuthenticateTokenSuccessResponse {
            auth_ctx: Some(auth_ctx.into()),
        })
    }

    async fn get_auth_details_for_environment_internal(
        &self,
        request: GetAuthDetailsForEnvironmentRequest,
    ) -> Result<GetAuthDetailsForEnvironmentSuccessResponse, GrpcApiError> {
        let environment_id: EnvironmentId = request
            .environment_id
            .ok_or("missing environment_id field")?
            .try_into()?;

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;

        let environment = self
            .environment_service
            .get(environment_id, request.include_deleted, &auth_ctx)
            .await?;

        let auth_details_for_environment = AuthDetailsForEnvironment {
            account_id_owning_environment: environment.owner_account_id,
            environment_roles_from_shares: environment.roles_from_active_shares,
        };

        Ok(GetAuthDetailsForEnvironmentSuccessResponse {
            auth_details_for_environment: Some(auth_details_for_environment.into()),
        })
    }

    async fn get_resource_limits_internal(
        &self,
        request: GetResourceLimitsRequest,
    ) -> Result<GetResourceLimitsSuccessResponse, GrpcApiError> {
        let account_id: AccountId = request
            .account_id
            .ok_or("missing account_id field")?
            .try_into()?;
        let limits = self
            .account_usage_service
            .get_resouce_limits(account_id, &AuthCtx::System)
            .await?;
        Ok(GetResourceLimitsSuccessResponse {
            limits: Some(limits.into()),
        })
    }

    async fn update_worker_limit_internal(
        &self,
        request: UpdateWorkerLimitRequest,
    ) -> Result<EmptySuccessResponse, GrpcApiError> {
        let account_id: AccountId = request
            .account_id
            .ok_or("missing account_id field")?
            .try_into()?;
        if request.added {
            self.account_usage_service
                .add_worker(account_id, &AuthCtx::System)
                .await?;
        } else {
            self.account_usage_service
                .remove_worker(account_id, &AuthCtx::System)
                .await?;
        }
        Ok(EmptySuccessResponse {})
    }

    async fn update_worker_connection_limit_internal(
        &self,
        request: UpdateWorkerConnectionLimitRequest,
    ) -> Result<EmptySuccessResponse, GrpcApiError> {
        let account_id: AccountId = request
            .account_id
            .ok_or("missing account_id field")?
            .try_into()?;
        if request.added {
            self.account_usage_service
                .add_worker_connection(account_id, &AuthCtx::System)
                .await?;
        } else {
            self.account_usage_service
                .remove_worker_connection(account_id, &AuthCtx::System)
                .await?;
        }
        Ok(EmptySuccessResponse {})
    }

    async fn batch_update_resource_usage_internal(
        &self,
        request: BatchUpdateResourceUsageRequest,
    ) -> Result<BatchUpdateResourceUsageSuccessResponse, GrpcApiError> {
        let updates: HashMap<AccountId, ResourceUsageUpdate> = request
            .updates
            .into_iter()
            .map(|u| {
                let account_id = u.account_id.ok_or("missing account_id field")?.try_into()?;
                Ok::<_, GrpcApiError>((
                    account_id,
                    ResourceUsageUpdate {
                        fuel_delta: u.fuel_delta,
                        http_call_count_delta: u.http_call_count_delta,
                        rpc_call_count_delta: u.rpc_call_count_delta,
                    },
                ))
            })
            .collect::<Result<_, _>>()?;

        let account_resource_limits = self
            .account_usage_service
            .update_resource_usage(updates, &AuthCtx::System)
            .await?;

        Ok(BatchUpdateResourceUsageSuccessResponse {
            account_resource_limits: Some(account_resource_limits.into()),
        })
    }

    async fn download_component_internal(
        &self,
        request: DownloadComponentRequest,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, GrpcApiError> {
        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let revision: ComponentRevision = request.revision.try_into()?;

        let result = self
            .component_service
            .download_component_wasm(component_id, revision, true, &AuthCtx::System)
            .await?;
        Ok(result)
    }

    async fn get_component_metadata_internal(
        &self,
        request: GetComponentMetadataRequest,
    ) -> Result<GetComponentMetadataSuccessResponse, GrpcApiError> {
        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let revision: ComponentRevision = request.revision.try_into()?;

        let component = self
            .component_service
            .get_component_revision(component_id, revision, true, &AuthCtx::System)
            .await?;

        Ok(GetComponentMetadataSuccessResponse {
            component: Some(component.into()),
        })
    }

    async fn get_deployed_component_metadata_internal(
        &self,
        request: GetDeployedComponentMetadataRequest,
    ) -> Result<GetDeployedComponentMetadataSuccessResponse, GrpcApiError> {
        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let component = self
            .component_service
            .get_deployed_component(component_id, &AuthCtx::System)
            .await?;

        Ok(GetDeployedComponentMetadataSuccessResponse {
            component: Some(component.into()),
        })
    }

    async fn get_all_deployed_component_revisions_internal(
        &self,
        request: GetAllDeployedComponentRevisionsRequest,
    ) -> Result<GetAllDeployedComponentRevisionsSuccessResponse, GrpcApiError> {
        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let components = self
            .component_service
            .get_all_deployed_component_versions(component_id, &AuthCtx::System)
            .await?;

        Ok(GetAllDeployedComponentRevisionsSuccessResponse {
            components: components.into_iter().map(|c| c.into()).collect(),
        })
    }

    async fn resolve_component_internal(
        &self,
        request: ResolveComponentRequest,
    ) -> Result<ResolveComponentSuccessResponse, GrpcApiError> {
        let component_reference = request.component_slug;

        let resolving_account_id: AccountId = request
            .resolving_account_id
            .ok_or("missing resolving_account_id field")?
            .try_into()?;

        let resolving_application_id: ApplicationId = request
            .resolving_application_id
            .ok_or("missing resolving_application_id field")?
            .try_into()?;

        let resolving_environment_id: EnvironmentId = request
            .resolving_environment_id
            .ok_or("missing resolving_environment_id field")?
            .try_into()?;

        let component = self
            .component_resolver_service
            .resolve_deployed_component(
                component_reference,
                resolving_environment_id,
                resolving_application_id,
                resolving_account_id,
                &AuthCtx::System,
            )
            .await?;

        Ok(ResolveComponentSuccessResponse {
            component: Some(component.into()),
        })
    }

    async fn get_all_agent_types_internal(
        &self,
        request: GetAllAgentTypesRequest,
    ) -> Result<GetAllAgentTypesSuccessResponse, GrpcApiError> {
        let environment_id: EnvironmentId = request
            .environment_id
            .ok_or("missing environment_id field")?
            .try_into()?;

        let agent_types = self
            .deployment_service
            .list_deployed_agent_types(environment_id)
            .await?;

        Ok(GetAllAgentTypesSuccessResponse {
            agent_types: agent_types
                .into_iter()
                .map(|at| RegisteredAgentType::from(at).into())
                .collect(),
        })
    }

    async fn get_agent_type_internal(
        &self,
        request: GetAgentTypeRequest,
    ) -> Result<GetAgentTypeSuccessResponse, GrpcApiError> {
        let environment_id: EnvironmentId = request
            .environment_id
            .ok_or("missing environment_id field")?
            .try_into()?;

        let agent_type_name = AgentTypeName(request.agent_type);

        let agent_type = self
            .deployment_service
            .get_deployed_agent_type(environment_id, &agent_type_name)
            .await?;

        Ok(GetAgentTypeSuccessResponse {
            agent_type: Some(RegisteredAgentType::from(agent_type).into()),
        })
    }

    async fn get_active_routes_for_domain_internal(
        &self,
        request: GetActiveRoutesForDomainRequest,
    ) -> Result<GetActiveRoutesForDomainSuccessResponse, GrpcApiError> {
        let domain: Domain = Domain(request.domain);

        let compiled_routes = self
            .deployed_routes_service
            .get_currently_active_compiled_routes(&domain)
            .await?;

        Ok(GetActiveRoutesForDomainSuccessResponse {
            compiled_routes: Some(compiled_routes.into()),
        })
    }

    async fn get_active_mcp_routes_for_domain_internal(
        &self,
        request: GetActiveMcpForDomainRequest,
    ) -> Result<GetActiveMcpForDomainSuccessResponse, GrpcApiError> {
        let domain: Domain = Domain(request.domain);

        let compiled_mcp = self
            .deployed_mcp_service
            .get_currently_active_mcp(&domain)
            .await?;

        Ok(GetActiveMcpForDomainSuccessResponse {
            compiled_mcp: Some(golem_api_grpc::proto::golem::mcp::CompiledMcp::from(
                compiled_mcp,
            )),
        })
    }

    async fn get_agent_deployments_internal(
        &self,
        request: GetCurrentEnvironmentStateRequest,
    ) -> Result<GetCurrentEnvironmentStateSuccessResponse, GrpcApiError> {
        let environment_id: EnvironmentId = request
            .environment_id
            .ok_or("missing environment_id field")?
            .try_into()?;

        let environment_state = self
            .environment_state_service
            .get_environment_state(environment_id)
            .await?;

        Ok(GetCurrentEnvironmentStateSuccessResponse {
            environment_state: Some(environment_state.into()),
        })
    }

    async fn get_resource_definition_by_id_internal(
        &self,
        request: GetResourceDefinitionByIdRequest,
    ) -> Result<GetResourceDefinitionByIdSuccessResponse, GrpcApiError> {
        let resource_definition_id: ResourceDefinitionId = request
            .resource_definition_id
            .ok_or("missing resource_definition_id field")?
            .try_into()?;

        let resource_definition = self
            .resource_definition_service
            .get(resource_definition_id, &AuthCtx::system())
            .await?;

        Ok(GetResourceDefinitionByIdSuccessResponse {
            resource_definition: Some(resource_definition.into()),
        })
    }

    async fn get_resource_definition_by_name_internal(
        &self,
        request: GetResourceDefinitionByNameRequest,
    ) -> Result<GetResourceDefinitionByNameSuccessResponse, GrpcApiError> {
        let environment_id: EnvironmentId = request
            .environment_id
            .ok_or("missing environment_id field")?
            .try_into()?;

        let resource_name = ResourceName(request.name);

        let resource_definition = self
            .resource_definition_service
            .get_in_environment(environment_id, &resource_name, &AuthCtx::system())
            .await?;

        Ok(GetResourceDefinitionByNameSuccessResponse {
            resource_definition: Some(resource_definition.into()),
        })
    }

    async fn resolve_agent_type_by_names_internal(
        &self,
        request: ResolveAgentTypeByNamesRequest,
    ) -> Result<ResolveAgentTypeByNamesSuccessResponse, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;
        let app_name = ApplicationName(request.app_name);
        let environment_name = EnvironmentName(request.environment_name);
        let agent_type_name = AgentTypeName(request.agent_type_name);
        let deployment_revision = request
            .deployment_revision
            .map(DeploymentRevision::try_from)
            .transpose()
            .map_err(|e: String| e)?;

        let resolved = self
            .deployment_service
            .resolve_agent_type_by_names(
                &app_name,
                &environment_name,
                &agent_type_name,
                deployment_revision,
                request.owner_account_email.as_deref(),
                &auth_ctx,
            )
            .await?;

        Ok(ResolveAgentTypeByNamesSuccessResponse {
            agent_type: Some(resolved.registered_agent_type.into()),
            environment_id: Some(resolved.environment_id.into()),
            deployment_revision: resolved.deployment_revision.get(),
            current_deployment_revision: resolved.current_deployment_revision.map(|r| r.get()),
        })
    }
}

#[async_trait]
impl golem_api_grpc::proto::golem::registry::v1::registry_service_server::RegistryService
    for RegistryServiceGrpcApi
{
    async fn authenticate_token(
        &self,
        request: Request<AuthenticateTokenRequest>,
    ) -> Result<Response<AuthenticateTokenResponse>, tonic::Status> {
        let record = recorded_grpc_api_request!("authenticate_token",);

        let response = match self
            .authenticate_token_internal(request.into_inner())
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => authenticate_token_response::Result::Success(result),
            Err(error) => authenticate_token_response::Result::Error(error.into()),
        };

        Ok(Response::new(AuthenticateTokenResponse {
            result: Some(response),
        }))
    }

    async fn get_auth_details_for_environment(
        &self,
        request: Request<GetAuthDetailsForEnvironmentRequest>,
    ) -> Result<Response<GetAuthDetailsForEnvironmentResponse>, tonic::Status> {
        let record = recorded_grpc_api_request!("get_auth_details_for_environment",);

        let response = match self
            .get_auth_details_for_environment_internal(request.into_inner())
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_auth_details_for_environment_response::Result::Success(result),
            Err(error) => get_auth_details_for_environment_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetAuthDetailsForEnvironmentResponse {
            result: Some(response),
        }))
    }

    async fn get_resource_limits(
        &self,
        request: Request<GetResourceLimitsRequest>,
    ) -> Result<Response<GetResourceLimitsResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_resource_limits",
            account_id = AccountId::render_proto(request.account_id)
        );

        let response = match self
            .get_resource_limits_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_resource_limits_response::Result::Success(result),
            Err(error) => get_resource_limits_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetResourceLimitsResponse {
            result: Some(response),
        }))
    }

    async fn update_worker_limit(
        &self,
        request: Request<UpdateWorkerLimitRequest>,
    ) -> Result<Response<UpdateWorkerLimitResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "update_worker_limit",
            account_id = AccountId::render_proto(request.account_id)
        );

        let response = match self
            .update_worker_limit_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => update_worker_limit_response::Result::Success(result),
            Err(error) => update_worker_limit_response::Result::Error(error.into()),
        };

        Ok(Response::new(UpdateWorkerLimitResponse {
            result: Some(response),
        }))
    }

    async fn update_worker_connection_limit(
        &self,
        request: Request<UpdateWorkerConnectionLimitRequest>,
    ) -> Result<Response<UpdateWorkerConnectionLimitResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "update_worker_connection_limit",
            account_id = AccountId::render_proto(request.account_id)
        );

        let response = match self
            .update_worker_connection_limit_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => update_worker_connection_limit_response::Result::Success(result),
            Err(error) => update_worker_connection_limit_response::Result::Error(error.into()),
        };

        Ok(Response::new(UpdateWorkerConnectionLimitResponse {
            result: Some(response),
        }))
    }

    async fn batch_update_resource_usage(
        &self,
        request: Request<BatchUpdateResourceUsageRequest>,
    ) -> Result<Response<BatchUpdateResourceUsageResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("batch_update_resource_usage",);

        let response = match self
            .batch_update_resource_usage_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => batch_update_resource_usage_response::Result::Success(result),
            Err(error) => batch_update_resource_usage_response::Result::Error(error.into()),
        };

        Ok(Response::new(BatchUpdateResourceUsageResponse {
            result: Some(response),
        }))
    }

    type DownloadComponentStream =
        BoxStream<'static, Result<DownloadComponentResponse, tonic::Status>>;

    async fn download_component(
        &self,
        request: Request<DownloadComponentRequest>,
    ) -> Result<Response<Self::DownloadComponentStream>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "download_component",
            component_id = ComponentId::render_proto(request.component_id)
        );
        let stream: Self::DownloadComponentStream = match self
            .download_component_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(response) => {
                let stream = response.map(|content| {
                    let res = match content {
                        Ok(content) => DownloadComponentResponse {
                            result: Some(download_component_response::Result::SuccessChunk(
                                content,
                            )),
                        },
                        Err(_) => DownloadComponentResponse {
                            result: Some(download_component_response::Result::Error(
                                internal_error("Internal error: {e}"),
                            )),
                        },
                    };
                    Ok(res)
                });
                Box::pin(stream)
            }
            Err(err) => {
                let response = DownloadComponentResponse {
                    result: Some(download_component_response::Result::Error(err.into())),
                };
                Box::pin(tokio_stream::iter([Ok(response)]))
            }
        };

        Ok(Response::new(stream))
    }

    async fn get_component_metadata(
        &self,
        request: Request<GetComponentMetadataRequest>,
    ) -> Result<Response<GetComponentMetadataResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_component_metadata",
            component_id = ComponentId::render_proto(request.component_id),
            revision = request.revision
        );

        let response = match self
            .get_component_metadata_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_component_metadata_response::Result::Success(result),
            Err(error) => get_component_metadata_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetComponentMetadataResponse {
            result: Some(response),
        }))
    }

    async fn get_deployed_component_metadata(
        &self,
        request: Request<GetDeployedComponentMetadataRequest>,
    ) -> Result<Response<GetDeployedComponentMetadataResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_deployed_component_metadata",
            component_id = ComponentId::render_proto(request.component_id)
        );

        let response = match self
            .get_deployed_component_metadata_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_deployed_component_metadata_response::Result::Success(result),
            Err(error) => get_deployed_component_metadata_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetDeployedComponentMetadataResponse {
            result: Some(response),
        }))
    }

    async fn get_all_deployed_component_revisions(
        &self,
        request: Request<GetAllDeployedComponentRevisionsRequest>,
    ) -> Result<Response<GetAllDeployedComponentRevisionsResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_all_deployed_component_revisions",
            component_id = ComponentId::render_proto(request.component_id)
        );

        let response = match self
            .get_all_deployed_component_revisions_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_all_deployed_component_revisions_response::Result::Success(result),
            Err(error) => {
                get_all_deployed_component_revisions_response::Result::Error(error.into())
            }
        };

        Ok(Response::new(GetAllDeployedComponentRevisionsResponse {
            result: Some(response),
        }))
    }

    async fn resolve_component(
        &self,
        request: Request<ResolveComponentRequest>,
    ) -> Result<Response<ResolveComponentResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "resolve_component",
            component_slug = &request.component_slug,
            account_id = AccountId::render_proto(request.resolving_account_id),
            application_id = ApplicationId::render_proto(request.resolving_application_id),
            environment_id = EnvironmentId::render_proto(request.resolving_environment_id)
        );

        let response = match self
            .resolve_component_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => resolve_component_response::Result::Success(result),
            Err(error) => resolve_component_response::Result::Error(error.into()),
        };

        Ok(Response::new(ResolveComponentResponse {
            result: Some(response),
        }))
    }

    async fn get_all_agent_types(
        &self,
        request: Request<GetAllAgentTypesRequest>,
    ) -> Result<Response<GetAllAgentTypesResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_all_agent_types",
            environment_id = EnvironmentId::render_proto(request.environment_id)
        );

        let response = match self
            .get_all_agent_types_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_all_agent_types_response::Result::Success(result),
            Err(error) => get_all_agent_types_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetAllAgentTypesResponse {
            result: Some(response),
        }))
    }

    async fn get_agent_type(
        &self,
        request: Request<GetAgentTypeRequest>,
    ) -> Result<Response<GetAgentTypeResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_agent_type",
            environment_id = EnvironmentId::render_proto(request.environment_id),
            agent_type = &request.agent_type
        );

        let response = match self
            .get_agent_type_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_agent_type_response::Result::Success(result),
            Err(error) => get_agent_type_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetAgentTypeResponse {
            result: Some(response),
        }))
    }

    async fn resolve_agent_type_by_names(
        &self,
        request: Request<ResolveAgentTypeByNamesRequest>,
    ) -> Result<Response<ResolveAgentTypeByNamesResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "resolve_agent_type_by_names",
            app_name = &request.app_name,
            environment_name = &request.environment_name,
            agent_type_name = &request.agent_type_name,
        );

        let response = match self
            .resolve_agent_type_by_names_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => resolve_agent_type_by_names_response::Result::Success(result),
            Err(error) => resolve_agent_type_by_names_response::Result::Error(error.into()),
        };

        Ok(Response::new(ResolveAgentTypeByNamesResponse {
            result: Some(response),
        }))
    }

    async fn get_active_routes_for_domain(
        &self,
        request: Request<GetActiveRoutesForDomainRequest>,
    ) -> Result<Response<GetActiveRoutesForDomainResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_active_routes_for_domain",
            get_active_routes_for_domain = request.domain,
        );

        let response = match self
            .get_active_routes_for_domain_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_active_routes_for_domain_response::Result::Success(result),
            Err(error) => get_active_routes_for_domain_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetActiveRoutesForDomainResponse {
            result: Some(response),
        }))
    }

    async fn get_active_mcp_for_domain(
        &self,
        request: Request<GetActiveMcpForDomainRequest>,
    ) -> Result<Response<GetActiveMcpForDomainResponse>, Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_active_mcp_for_domain",
            get_active_mcp_for_domain = request.domain,
        );

        let response = match self
            .get_active_mcp_routes_for_domain_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_active_mcp_for_domain_response::Result::Success(result),
            Err(error) => get_active_mcp_for_domain_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetActiveMcpForDomainResponse {
            result: Some(response),
        }))
    }

    async fn get_current_environment_state(
        &self,
        request: Request<GetCurrentEnvironmentStateRequest>,
    ) -> Result<Response<GetCurrentEnvironmentStateResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_current_environment_state",
            environment_id = EnvironmentId::render_proto(request.environment_id),
        );

        let response = match self
            .get_agent_deployments_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_current_environment_state_response::Result::Success(result),
            Err(error) => get_current_environment_state_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetCurrentEnvironmentStateResponse {
            result: Some(response),
        }))
    }

    async fn get_resource_definition_by_id(
        &self,
        request: Request<GetResourceDefinitionByIdRequest>,
    ) -> Result<Response<GetResourceDefinitionByIdResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_resource_definition_by_id",
            resource_definition_id =
                ResourceDefinitionId::render_proto(request.resource_definition_id),
        );

        let response = match self
            .get_resource_definition_by_id_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_resource_definition_by_id_response::Result::Success(result),
            Err(error) => get_resource_definition_by_id_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetResourceDefinitionByIdResponse {
            result: Some(response),
        }))
    }

    async fn get_resource_definition_by_name(
        &self,
        request: Request<GetResourceDefinitionByNameRequest>,
    ) -> Result<Response<GetResourceDefinitionByNameResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_resource_definition_by_name",
            environment_id = EnvironmentId::render_proto(request.environment_id),
        );

        let response = match self
            .get_resource_definition_by_name_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_resource_definition_by_name_response::Result::Success(result),
            Err(error) => get_resource_definition_by_name_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetResourceDefinitionByNameResponse {
            result: Some(response),
        }))
    }

    type SubscribeRegistryInvalidationsStream = std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<RegistryInvalidationEvent, tonic::Status>> + Send>,
    >;

    async fn subscribe_registry_invalidations(
        &self,
        request: Request<SubscribeRegistryInvalidationsRequest>,
    ) -> Result<Response<Self::SubscribeRegistryInvalidationsStream>, Status> {
        let last_seen = request.into_inner().last_seen_event_id;

        let stream = crate::services::registry_change_notifier::subscribe_registry_invalidations(
            self.registry_change_repo.clone(),
            self.registry_change_notifier.as_ref(),
            last_seen,
        );

        Ok(Response::new(Box::pin(stream)))
    }
}

fn internal_error(error: &str) -> RegistryServiceError {
    RegistryServiceError {
        error: Some(registry_service_error::Error::InternalError(
            golem_api_grpc::proto::golem::common::ErrorBody {
                error: error.to_string(),
                code: golem_common::base_model::api::error_code::INTERNAL_UNKNOWN.to_string(),
            },
        )),
    }
}
