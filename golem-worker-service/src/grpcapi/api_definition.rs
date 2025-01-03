// Copyright 2024-2025 Golem Cloud
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

use std::sync::Arc;

use async_trait::async_trait;
use tracing::Instrument;

use golem_api_grpc::proto::golem::apidefinition::{
    ApiDefinition as GrpcApiDefinition, ApiDefinitionList,
};
use golem_api_grpc::proto::golem::{
    apidefinition::v1::{
        api_definition_error, api_definition_service_server::ApiDefinitionService,
        create_api_definition_request, create_api_definition_response,
        delete_api_definition_response, get_all_api_definitions_response,
        get_api_definition_response, get_api_definition_versions_response,
        update_api_definition_request, update_api_definition_response, ApiDefinitionError,
        CreateApiDefinitionRequest, CreateApiDefinitionResponse, DeleteApiDefinitionRequest,
        DeleteApiDefinitionResponse, GetAllApiDefinitionsRequest, GetAllApiDefinitionsResponse,
        GetApiDefinitionRequest, GetApiDefinitionResponse, GetApiDefinitionVersionsRequest,
        GetApiDefinitionVersionsResponse, UpdateApiDefinitionRequest, UpdateApiDefinitionResponse,
    },
    common::{Empty, ErrorBody, ErrorsBody},
};
use golem_common::grpc::{
    proto_api_definition_draft_string, proto_api_definition_id_string,
    proto_api_definition_kind_string, proto_api_definition_version_string,
};
use golem_common::recorded_grpc_api_request;
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::api::ApiDefinitionTraceErrorKind;
use golem_worker_service_base::gateway_api_definition::http::OpenApiHttpApiDefinitionRequest;
use golem_worker_service_base::gateway_api_definition::{ApiDefinitionId, ApiVersion};

#[derive(Clone)]
pub struct GrpcApiDefinitionService {
    definition_service: Arc<
        dyn golem_worker_service_base::service::gateway::api_definition::ApiDefinitionService<
                EmptyAuthCtx,
                DefaultNamespace,
            > + Sync
            + Send,
    >,
}

impl GrpcApiDefinitionService {
    pub fn new(
        definition_service: Arc<
            dyn golem_worker_service_base::service::gateway::api_definition::ApiDefinitionService<
                    EmptyAuthCtx,
                    DefaultNamespace,
                > + Sync
                + Send,
        >,
    ) -> Self {
        Self { definition_service }
    }
}

#[async_trait]
impl ApiDefinitionService for GrpcApiDefinitionService {
    async fn create_api_definition(
        &self,
        request: tonic::Request<CreateApiDefinitionRequest>,
    ) -> Result<tonic::Response<CreateApiDefinitionResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "create_api_definition",
            kind = proto_api_definition_kind_string(&request.api_definition),
            version = proto_api_definition_version_string(&request.api_definition),
            draft = proto_api_definition_draft_string(&request.api_definition),
        );

        let result = match self
            .create_api_definition(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(create_api_definition_response::Result::Success(result)),
            Err(error) => record.fail(
                create_api_definition_response::Result::Error(error.clone()),
                &ApiDefinitionTraceErrorKind(&error),
            ),
        };

        Ok(tonic::Response::new(CreateApiDefinitionResponse {
            result: Some(result),
        }))
    }

    async fn update_api_definition(
        &self,
        request: tonic::Request<UpdateApiDefinitionRequest>,
    ) -> Result<tonic::Response<UpdateApiDefinitionResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "update_api_definition",
            kind = proto_api_definition_kind_string(&request.api_definition),
            api_definition_id = proto_api_definition_id_string(&request.api_definition),
            version = proto_api_definition_version_string(&request.api_definition),
            draft = proto_api_definition_draft_string(&request.api_definition),
        );

        let result = match self
            .update_api_definition(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(update_api_definition_response::Result::Success(result)),
            Err(error) => record.fail(
                update_api_definition_response::Result::Error(error.clone()),
                &ApiDefinitionTraceErrorKind(&error),
            ),
        };

        Ok(tonic::Response::new(UpdateApiDefinitionResponse {
            result: Some(result),
        }))
    }

    async fn get_api_definition(
        &self,
        request: tonic::Request<GetApiDefinitionRequest>,
    ) -> Result<tonic::Response<GetApiDefinitionResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_api_definition",
            api_definition_id = request
                .api_definition_id
                .as_ref()
                .map(|id| { id.value.clone() }),
            version = request.version,
        );

        let result = match self
            .get_api_definition(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(get_api_definition_response::Result::Success(result)),
            Err(error) => record.fail(
                get_api_definition_response::Result::Error(error.clone()),
                &ApiDefinitionTraceErrorKind(&error),
            ),
        };

        Ok(tonic::Response::new(GetApiDefinitionResponse {
            result: Some(result),
        }))
    }

    async fn get_api_definition_versions(
        &self,
        request: tonic::Request<GetApiDefinitionVersionsRequest>,
    ) -> Result<tonic::Response<GetApiDefinitionVersionsResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "get_api_definition_versions",
            api_definition_id = request
                .api_definition_id
                .as_ref()
                .map(|id| { id.value.clone() }),
        );

        let result = match self
            .get_all_api_definition_versions(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(definitions) => {
                record.succeed(get_api_definition_versions_response::Result::Success(
                    ApiDefinitionList { definitions },
                ))
            }
            Err(error) => record.fail(
                get_api_definition_versions_response::Result::Error(error.clone()),
                &ApiDefinitionTraceErrorKind(&error),
            ),
        };

        Ok(tonic::Response::new(GetApiDefinitionVersionsResponse {
            result: Some(result),
        }))
    }

    async fn get_all_api_definitions(
        &self,
        request: tonic::Request<GetAllApiDefinitionsRequest>,
    ) -> Result<tonic::Response<GetAllApiDefinitionsResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!("get_all_api_definitions",);

        let result = match self
            .get_all_api_definitions(request)
            .instrument(record.span.clone())
            .await
        {
            Ok(definitions) => record.succeed(get_all_api_definitions_response::Result::Success(
                ApiDefinitionList { definitions },
            )),
            Err(error) => record.fail(
                get_all_api_definitions_response::Result::Error(error.clone()),
                &ApiDefinitionTraceErrorKind(&error),
            ),
        };

        Ok(tonic::Response::new(GetAllApiDefinitionsResponse {
            result: Some(result),
        }))
    }

    async fn delete_api_definition(
        &self,
        request: tonic::Request<DeleteApiDefinitionRequest>,
    ) -> Result<tonic::Response<DeleteApiDefinitionResponse>, tonic::Status> {
        let request = request.into_inner();
        let record = recorded_grpc_api_request!(
            "delete_api_definition",
            api_definition_id = request
                .api_definition_id
                .as_ref()
                .map(|id| { id.value.clone() }),
            version = request.version,
        );

        let result = match self.delete_api_definition(request).await {
            Ok(_) => record.succeed(delete_api_definition_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                delete_api_definition_response::Result::Error(error.clone()),
                &ApiDefinitionTraceErrorKind(&error),
            ),
        };

        Ok(tonic::Response::new(DeleteApiDefinitionResponse {
            result: Some(result),
        }))
    }
}

impl GrpcApiDefinitionService {
    async fn create_api_definition(
        &self,
        request: CreateApiDefinitionRequest,
    ) -> Result<GrpcApiDefinition, ApiDefinitionError> {
        let definition = request
            .api_definition
            .ok_or(bad_request("Missing Api Definition"))?;

        let internal_definition = match definition {
            create_api_definition_request::ApiDefinition::Definition(definition) => {
                definition.clone().try_into().map_err(bad_request)?
            }
            create_api_definition_request::ApiDefinition::Openapi(definition) => {
                let value = OpenApiHttpApiDefinitionRequest(
                    serde_json::from_str(&definition).map_err(|_| bad_request("Invalid JSON"))?,
                );

                value
                    .to_http_api_definition_request()
                    .map_err(bad_request)?
            }
        };

        let result = self
            .definition_service
            .create(
                &internal_definition,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let definition =
            golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition::from(
                result,
            )
            .try_into()
            .map_err(internal_error)?;

        Ok(definition)
    }

    async fn update_api_definition(
        &self,
        request: UpdateApiDefinitionRequest,
    ) -> Result<GrpcApiDefinition, ApiDefinitionError> {
        let definition = request
            .api_definition
            .ok_or(bad_request("Missing Api Definition"))?;

        let internal_definition = match definition {
            update_api_definition_request::ApiDefinition::Definition(definition) => {
                definition.clone().try_into().map_err(bad_request)?
            }
            update_api_definition_request::ApiDefinition::Openapi(definition) => {
                let value = OpenApiHttpApiDefinitionRequest(
                    serde_json::from_str(&definition).map_err(|_| bad_request("Invalid JSON"))?,
                );

                value
                    .to_http_api_definition_request()
                    .map_err(bad_request)?
            }
        };

        let result = self
            .definition_service
            .update(
                &internal_definition,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let definition =
            golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition::from(
                result,
            )
            .try_into()
            .map_err(internal_error)?;

        Ok(definition)
    }

    async fn get_api_definition(
        &self,
        request: GetApiDefinitionRequest,
    ) -> Result<GrpcApiDefinition, ApiDefinitionError> {
        let api_definition_id = request
            .api_definition_id
            .ok_or(bad_request("Missing Api Definition Id"))?;
        let api_definition_id = ApiDefinitionId(api_definition_id.value);
        let version = ApiVersion(request.version);

        let definition = self
            .definition_service
            .get(
                &api_definition_id,
                &version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?
            .map(golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition::from)
            .ok_or_else(|| {
                not_found(format!(
                    "Api Definition with id: {} and version: {} not found",
                    api_definition_id.0, version.0
                ))
            })?;

        let definition = definition.try_into().map_err(internal_error)?;

        Ok(definition)
    }

    async fn get_all_api_definition_versions(
        &self,
        request: GetApiDefinitionVersionsRequest,
    ) -> Result<Vec<GrpcApiDefinition>, ApiDefinitionError> {
        let api_definition_id = get_api_definition_id(request.api_definition_id)?;

        let definitions = self
            .definition_service
            .get_all_versions(
                &api_definition_id,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        let definitions = definitions
            .into_iter()
            .map(golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition::from)
            .map(|d| d.try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal_error)?;

        Ok(definitions)
    }

    async fn get_all_api_definitions(
        &self,
        _request: GetAllApiDefinitionsRequest,
    ) -> Result<Vec<GrpcApiDefinition>, ApiDefinitionError> {
        let definitions = self
            .definition_service
            .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
            .await?;

        let definitions = definitions
            .into_iter()
            .map(golem_worker_service_base::gateway_api_definition::http::HttpApiDefinition::from)
            .map(|d| d.try_into())
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal_error)?;

        Ok(definitions)
    }

    async fn delete_api_definition(
        &self,
        request: DeleteApiDefinitionRequest,
    ) -> Result<(), ApiDefinitionError> {
        let api_definition_id = get_api_definition_id(request.api_definition_id)?;
        let version = ApiVersion(request.version);

        self.definition_service
            .delete(
                &api_definition_id,
                &version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await?;

        Ok(())
    }
}

fn get_api_definition_id(
    id: Option<golem_api_grpc::proto::golem::apidefinition::ApiDefinitionId>,
) -> Result<ApiDefinitionId, ApiDefinitionError> {
    id.map(|id| ApiDefinitionId(id.value))
        .ok_or(bad_request("Missing Api Definition Id"))
}

fn bad_request(error: impl Into<String>) -> ApiDefinitionError {
    ApiDefinitionError {
        error: Some(api_definition_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.into()],
        })),
    }
}

fn not_found(error: impl Into<String>) -> ApiDefinitionError {
    ApiDefinitionError {
        error: Some(api_definition_error::Error::NotFound(ErrorBody {
            error: error.into(),
        })),
    }
}

fn internal_error(error: impl Into<String>) -> ApiDefinitionError {
    ApiDefinitionError {
        error: Some(api_definition_error::Error::InternalError(ErrorBody {
            error: error.into(),
        })),
    }
}
