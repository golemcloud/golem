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

use async_trait::async_trait;
use tonic::{Request, Response};
use golem_api_grpc::proto::golem::registry::v1::{
    AuthenticateTokenRequest, AuthenticateTokenResponse, BatchUpdateFuelUsageRequest, BatchUpdateFuelUsageResponse, DownloadComponentRequest, DownloadComponentResponse, GetAgentTypeRequest, GetAgentTypeResponse, GetAllAgentTypesRequest, GetAllAgentTypesResponse, GetAllComponentVersionsRequest, GetAllComponentVersionsResponse, GetComponentMetadataRequest, GetComponentMetadataResponse, GetLatestComponentRequest, GetPluginRegistrationByIdRequest, GetPluginRegistrationByIdResponse, GetResourceLimitsRequest, GetResourceLimitsResponse, ResolveComponentRequest, ResolveComponentResponse, UpdateWorkerLimitRequest, UpdateWorkerLimitResponse
};
use futures::stream::BoxStream;

pub struct RegistryServiceGrpcApi {}

#[async_trait]
impl golem_api_grpc::proto::golem::registry::v1::registry_service_server::RegistryService for RegistryServiceGrpcApi {
    type DownloadComponentStream = BoxStream<'static, Result<DownloadComponentResponse, tonic::Status>>;

    async fn authenticate_token(&self, request: Request<AuthenticateTokenRequest>) -> Result<Response<AuthenticateTokenResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn get_resource_limits(&self, request: Request<GetResourceLimitsRequest>) -> Result<Response<GetResourceLimitsResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn update_worker_limit(&self, request: Request<UpdateWorkerLimitRequest>) -> Result<Response<UpdateWorkerLimitResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn update_worker_connection_limit(&self, request: Request<UpdateWorkerLimitRequest>) -> Result<Response<UpdateWorkerLimitResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn batch_update_fuel_usage(&self, request: Request<BatchUpdateFuelUsageRequest>) -> Result<Response<BatchUpdateFuelUsageResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn get_plugin_registration_by_id(&self, request: Request<GetPluginRegistrationByIdRequest>) -> Result<Response<GetPluginRegistrationByIdResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn download_component(&self, request: Request<DownloadComponentRequest>) -> Result<Response<Self::DownloadComponentStream>, tonic::Status> {
        unimplemented!()
    }

    async fn get_component_metadata(&self, request: Request<GetComponentMetadataRequest>) -> Result<Response<GetComponentMetadataResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn get_latest_component_metadata(&self, request: Request<GetLatestComponentRequest>) -> Result<Response<GetComponentMetadataResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn get_all_component_versions(&self, request: Request<GetAllComponentVersionsRequest>) -> Result<Response<GetAllComponentVersionsResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn resolve_component(&self, request: Request<ResolveComponentRequest>) -> Result<Response<ResolveComponentResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn get_all_agent_types(&self, request: Request<GetAllAgentTypesRequest>) -> Result<Response<GetAllAgentTypesResponse>, tonic::Status> {
        unimplemented!()
    }

    async fn get_agent_type(&self, request: Request<GetAgentTypeRequest>) -> Result<Response<GetAgentTypeResponse>, tonic::Status> {
        unimplemented!()
    }
}
