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

use super::error::GrpcApiError;
use crate::services::account_usage::AccountUsageService;
use crate::services::auth::AuthService;
use crate::services::component::ComponentService;
use crate::services::component_resolver::ComponentResolverService;
use crate::services::deployment::{DeployedRoutesService, DeploymentService};
use crate::services::environment::EnvironmentService;
use applying::Apply;
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use golem_api_grpc::proto::golem::common::Empty as EmptySuccessResponse;
use golem_api_grpc::proto::golem::registry::v1::{
    AuthenticateTokenRequest, AuthenticateTokenResponse, AuthenticateTokenSuccessResponse,
    BatchUpdateFuelUsageRequest, BatchUpdateFuelUsageResponse, BatchUpdateFuelUsageSuccessResponse,
    DownloadComponentRequest, DownloadComponentResponse, GetActiveRoutesForDomainRequest,
    GetActiveRoutesForDomainResponse, GetActiveRoutesForDomainSuccessResponse, GetAgentTypeRequest,
    GetAgentTypeResponse, GetAgentTypeSuccessResponse, GetAllAgentTypesRequest,
    GetAllAgentTypesResponse, GetAllAgentTypesSuccessResponse, GetAllComponentVersionsRequest,
    GetAllComponentVersionsResponse, GetAllComponentVersionsSuccessResponse,
    GetAuthDetailsForEnvironmentRequest, GetAuthDetailsForEnvironmentResponse,
    GetAuthDetailsForEnvironmentSuccessResponse, GetComponentMetadataRequest,
    GetComponentMetadataResponse, GetComponentMetadataSuccessResponse,
    GetLatestComponentMetadataRequest, GetLatestComponentMetadataResponse,
    GetLatestComponentMetadataSuccessResponse, GetResourceLimitsRequest, GetResourceLimitsResponse,
    GetResourceLimitsSuccessResponse, RegistryServiceError, ResolveComponentRequest,
    ResolveComponentResponse, ResolveComponentSuccessResponse, UpdateWorkerConnectionLimitRequest,
    UpdateWorkerConnectionLimitResponse, UpdateWorkerLimitRequest, UpdateWorkerLimitResponse,
    authenticate_token_response, batch_update_fuel_usage_response, download_component_response,
    get_active_routes_for_domain_response, get_agent_type_response, get_all_agent_types_response,
    get_all_component_versions_response, get_auth_details_for_environment_response,
    get_component_metadata_response, get_latest_component_metadata_response,
    get_resource_limits_response, registry_service_error, resolve_component_response,
    update_worker_connection_limit_response, update_worker_limit_response,
};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentDto, ComponentId, ComponentRevision};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentId;
use golem_common::recorded_grpc_api_request;
use golem_service_base::grpc::{
    proto_account_id_string, proto_application_id_string, proto_component_id_string,
    proto_environment_id_string,
};
use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment};
use std::collections::HashMap;
use std::sync::Arc;
use tonic::{Request, Response};
use tracing_futures::Instrument;

pub struct RegistryServiceGrpcApi {
    auth_service: Arc<AuthService>,
    environment_service: Arc<EnvironmentService>,
    account_usage_service: Arc<AccountUsageService>,
    component_service: Arc<ComponentService>,
    component_resolver_service: Arc<ComponentResolverService>,
    deployment_service: Arc<DeploymentService>,
    deployed_routes_service: Arc<DeployedRoutesService>,
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
    ) -> Self {
        Self {
            auth_service,
            environment_service,
            account_usage_service,
            component_service,
            component_resolver_service,
            deployment_service,
            deployed_routes_service,
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
            .get(&environment_id, request.include_deleted, &auth_ctx)
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
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;
        let account_id: AccountId = request
            .account_id
            .ok_or("missing account_id field")?
            .try_into()?;
        let limits = self
            .account_usage_service
            .get_resouce_limits(&account_id, &auth_ctx)
            .await?;
        Ok(GetResourceLimitsSuccessResponse {
            limits: Some(limits.into()),
        })
    }

    async fn update_worker_limit_internal(
        &self,
        request: UpdateWorkerLimitRequest,
    ) -> Result<EmptySuccessResponse, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;
        let account_id: AccountId = request
            .account_id
            .ok_or("missing account_id field")?
            .try_into()?;
        if request.added {
            self.account_usage_service
                .add_worker(&account_id, &auth_ctx)
                .await?;
        } else {
            self.account_usage_service
                .remove_worker(&account_id, &auth_ctx)
                .await?;
        }
        Ok(EmptySuccessResponse {})
    }

    async fn update_worker_connection_limit_internal(
        &self,
        request: UpdateWorkerConnectionLimitRequest,
    ) -> Result<EmptySuccessResponse, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;
        let account_id: AccountId = request
            .account_id
            .ok_or("missing account_id field")?
            .try_into()?;
        if request.added {
            self.account_usage_service
                .add_worker_connection(&account_id, &auth_ctx)
                .await?;
        } else {
            self.account_usage_service
                .remove_worker_connection(&account_id, &auth_ctx)
                .await?;
        }
        Ok(EmptySuccessResponse {})
    }

    async fn batch_update_fuel_usage_internal(
        &self,
        request: BatchUpdateFuelUsageRequest,
    ) -> Result<BatchUpdateFuelUsageSuccessResponse, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;

        let updates: HashMap<AccountId, i64> = request
            .updates
            .into_iter()
            .map(|u| {
                let account_id = u.account_id.ok_or("missing account_id field")?.try_into()?;
                Ok::<_, GrpcApiError>((account_id, u.value))
            })
            .collect::<Result<_, _>>()?;

        let account_resource_limits = self
            .account_usage_service
            .record_fuel_consumption(updates, &auth_ctx)
            .await?;

        Ok(BatchUpdateFuelUsageSuccessResponse {
            account_resource_limits: Some(account_resource_limits.into()),
        })
    }

    async fn download_component_internal(
        &self,
        request: DownloadComponentRequest,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;

        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let version = ComponentRevision(request.version);

        let result = self
            .component_service
            .download_component_wasm(&component_id, version, true, &auth_ctx)
            .await?;
        Ok(result)
    }

    async fn get_component_metadata_internal(
        &self,
        request: GetComponentMetadataRequest,
    ) -> Result<GetComponentMetadataSuccessResponse, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;

        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let version: ComponentRevision = ComponentRevision(request.version);

        let component = self
            .component_service
            .get_component_revision(&component_id, version, true, &auth_ctx)
            .await?;

        Ok(GetComponentMetadataSuccessResponse {
            component: Some(ComponentDto::from(component).into()),
        })
    }

    async fn get_latest_component_metadata_internal(
        &self,
        request: GetLatestComponentMetadataRequest,
    ) -> Result<GetLatestComponentMetadataSuccessResponse, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;

        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let component = self
            .component_service
            .get_deployed_component(&component_id, &auth_ctx)
            .await?;

        Ok(GetLatestComponentMetadataSuccessResponse {
            component: Some(ComponentDto::from(component).into()),
        })
    }

    async fn get_all_component_versions_internal(
        &self,
        request: GetAllComponentVersionsRequest,
    ) -> Result<GetAllComponentVersionsSuccessResponse, GrpcApiError> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;

        let component_id: ComponentId = request
            .component_id
            .ok_or("missing component_id field")?
            .try_into()?;

        let components = self
            .component_service
            .get_all_deployed_component_versions(&component_id, &auth_ctx)
            .await?;

        Ok(GetAllComponentVersionsSuccessResponse {
            components: components
                .into_iter()
                .map(|c| ComponentDto::from(c).into())
                .collect(),
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

        let auth_ctx: AuthCtx = request
            .auth_ctx
            .ok_or("missing auth_ctx field")?
            .try_into()?;

        let component = self
            .component_resolver_service
            .resolve_deployed_component(
                component_reference,
                &resolving_environment_id,
                &resolving_application_id,
                &resolving_account_id,
                &auth_ctx,
            )
            .await?;

        Ok(ResolveComponentSuccessResponse {
            component: Some(ComponentDto::from(component).into()),
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
            .list_deployed_agent_types(&environment_id)
            .await?;

        Ok(GetAllAgentTypesSuccessResponse {
            agent_types: agent_types.into_iter().map(|at| at.into()).collect(),
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

        let agent_type = self
            .deployment_service
            .get_deployed_agent_type(&environment_id, &request.agent_type)
            .await?;

        Ok(GetAgentTypeSuccessResponse {
            agent_type: Some(agent_type.into()),
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
            compiled_routes: Some(compiled_routes.try_into()?),
        })
    }
}

#[async_trait]
impl golem_api_grpc::proto::golem::registry::v1::registry_service_server::RegistryService
    for RegistryServiceGrpcApi
{
    type DownloadComponentStream =
        BoxStream<'static, Result<DownloadComponentResponse, tonic::Status>>;

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
            account_id = proto_account_id_string(&request.account_id)
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
            account_id = proto_account_id_string(&request.account_id)
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
            account_id = proto_account_id_string(&request.account_id)
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

    async fn batch_update_fuel_usage(
        &self,
        request: Request<BatchUpdateFuelUsageRequest>,
    ) -> Result<Response<BatchUpdateFuelUsageResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("batch_update_fuel_usage",);

        let response = match self
            .batch_update_fuel_usage_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => batch_update_fuel_usage_response::Result::Success(result),
            Err(error) => batch_update_fuel_usage_response::Result::Error(error.into()),
        };

        Ok(Response::new(BatchUpdateFuelUsageResponse {
            result: Some(response),
        }))
    }

    async fn download_component(
        &self,
        request: Request<DownloadComponentRequest>,
    ) -> Result<Response<Self::DownloadComponentStream>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "download_component",
            component_id = proto_component_id_string(&request.component_id)
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
            component_id = proto_component_id_string(&request.component_id),
            version = request.version
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

    async fn get_latest_component_metadata(
        &self,
        request: Request<GetLatestComponentMetadataRequest>,
    ) -> Result<Response<GetLatestComponentMetadataResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_latest_component_metadata",
            component_id = proto_component_id_string(&request.component_id)
        );

        let response = match self
            .get_latest_component_metadata_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_latest_component_metadata_response::Result::Success(result),
            Err(error) => get_latest_component_metadata_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetLatestComponentMetadataResponse {
            result: Some(response),
        }))
    }

    async fn get_all_component_versions(
        &self,
        request: Request<GetAllComponentVersionsRequest>,
    ) -> Result<Response<GetAllComponentVersionsResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_all_component_versions",
            component_id = proto_component_id_string(&request.component_id)
        );

        let response = match self
            .get_all_component_versions_internal(request)
            .instrument(record.span.clone())
            .await
            .apply(|r| record.result(r))
        {
            Ok(result) => get_all_component_versions_response::Result::Success(result),
            Err(error) => get_all_component_versions_response::Result::Error(error.into()),
        };

        Ok(Response::new(GetAllComponentVersionsResponse {
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
            account_id = proto_account_id_string(&request.resolving_account_id),
            application_id = proto_application_id_string(&request.resolving_application_id),
            environment_id = proto_environment_id_string(&request.resolving_environment_id)
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
            environment_id = proto_environment_id_string(&request.environment_id)
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
            environment_id = proto_environment_id_string(&request.environment_id),
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
}

fn internal_error(error: &str) -> RegistryServiceError {
    RegistryServiceError {
        error: Some(registry_service_error::Error::InternalError(
            golem_api_grpc::proto::golem::common::ErrorBody {
                error: error.to_string(),
            },
        )),
    }
}
