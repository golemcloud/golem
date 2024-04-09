use std::sync::Arc;

use async_trait::async_trait;

use golem_api_grpc::proto::golem::{
    apidefinition::{
        api_definition_error, api_definition_service_server::ApiDefinitionService,
        create_or_update_api_definition_request, create_or_update_response_api_definition_response,
        delete_api_definition_response, get_all_api_definitions_response,
        get_api_definition_response, get_api_definition_versions_response,
        ApiDefinition as GrpcApiDefinition, ApiDefinitionError, ApiDefinitionList,
        CreateOrUpdateApiDefinitionRequest, CreateOrUpdateResponseApiDefinitionResponse,
        DeleteApiDefinitionRequest, DeleteApiDefinitionResponse, GetAllApiDefinitionsRequest,
        GetAllApiDefinitionsResponse, GetApiDefinitionRequest, GetApiDefinitionResponse,
        GetApiDefinitionVersionsRequest, GetApiDefinitionVersionsResponse,
    },
    common::{Empty, ErrorBody, ErrorsBody},
};
use golem_worker_service_base::{
    api_definition::{
        http::{get_api_definition, HttpApiDefinition as CoreHttpApiDefinition},
        ApiDefinitionId, ApiVersion,
    },
    auth::{CommonNamespace, EmptyAuthCtx},
    service::http::http_api_definition_validator::RouteValidationError,
};

#[derive(Clone)]
pub struct GrpcApiDefinitionService {
    definition_service: DefinitionService,
}

type DefinitionService = Arc<
    dyn golem_worker_service_base::service::api_definition::ApiDefinitionService<
            EmptyAuthCtx,
            CommonNamespace,
            CoreHttpApiDefinition,
            RouteValidationError,
        > + Sync
        + Send,
>;

impl GrpcApiDefinitionService {
    pub fn new(definition_service: DefinitionService) -> Self {
        Self { definition_service }
    }
}

#[async_trait]
impl ApiDefinitionService for GrpcApiDefinitionService {
    async fn create_or_update_api_definition(
        &self,
        request: tonic::Request<CreateOrUpdateApiDefinitionRequest>,
    ) -> Result<tonic::Response<CreateOrUpdateResponseApiDefinitionResponse>, tonic::Status> {
        let result = match self
            .create_or_update_api_definition(request.into_inner())
            .await
        {
            Ok(result) => {
                create_or_update_response_api_definition_response::Result::Success(result)
            }
            Err(error) => create_or_update_response_api_definition_response::Result::Error(error),
        };

        Ok(tonic::Response::new(
            CreateOrUpdateResponseApiDefinitionResponse {
                result: Some(result),
            },
        ))
    }

    async fn get_api_definition(
        &self,
        request: tonic::Request<GetApiDefinitionRequest>,
    ) -> Result<tonic::Response<GetApiDefinitionResponse>, tonic::Status> {
        let result = match self.get_api_definition(request.into_inner()).await {
            Ok(result) => get_api_definition_response::Result::Success(result),
            Err(error) => get_api_definition_response::Result::Error(error),
        };

        Ok(tonic::Response::new(GetApiDefinitionResponse {
            result: Some(result),
        }))
    }

    async fn get_api_definition_versions(
        &self,
        request: tonic::Request<GetApiDefinitionVersionsRequest>,
    ) -> Result<tonic::Response<GetApiDefinitionVersionsResponse>, tonic::Status> {
        let result = match self
            .get_all_api_definition_versions(request.into_inner())
            .await
        {
            Ok(definitions) => {
                get_api_definition_versions_response::Result::Success(ApiDefinitionList {
                    definitions,
                })
            }
            Err(error) => get_api_definition_versions_response::Result::Error(error),
        };

        Ok(tonic::Response::new(GetApiDefinitionVersionsResponse {
            result: Some(result),
        }))
    }

    async fn get_all_api_definitions(
        &self,
        request: tonic::Request<GetAllApiDefinitionsRequest>,
    ) -> std::result::Result<tonic::Response<GetAllApiDefinitionsResponse>, tonic::Status> {
        let result = match self.get_all_api_definitions(request.into_inner()).await {
            Ok(definitions) => {
                get_all_api_definitions_response::Result::Success(ApiDefinitionList { definitions })
            }
            Err(error) => get_all_api_definitions_response::Result::Error(error),
        };

        Ok(tonic::Response::new(GetAllApiDefinitionsResponse {
            result: Some(result),
        }))
    }

    async fn delete_api_definition(
        &self,
        request: tonic::Request<DeleteApiDefinitionRequest>,
    ) -> Result<tonic::Response<DeleteApiDefinitionResponse>, tonic::Status> {
        let result = match self.delete_api_definition(request.into_inner()).await {
            Ok(_) => delete_api_definition_response::Result::Success(Empty {}),
            Err(error) => delete_api_definition_response::Result::Error(error),
        };

        Ok(tonic::Response::new(DeleteApiDefinitionResponse {
            result: Some(result),
        }))
    }
}

impl GrpcApiDefinitionService {
    async fn create_or_update_api_definition(
        &self,
        request: CreateOrUpdateApiDefinitionRequest,
    ) -> Result<GrpcApiDefinition, ApiDefinitionError> {
        let definition = request
            .api_definition
            .ok_or(bad_request("Missing Api Definition"))?;

        let internal_definition = match definition {
            create_or_update_api_definition_request::ApiDefinition::Definition(definition) => {
                definition.clone().try_into().map_err(bad_request)?
            }
            create_or_update_api_definition_request::ApiDefinition::Openapi(definition) => {
                let value =
                    serde_json::from_str(&definition).map_err(|_| bad_request("Invalid JSON"))?;

                get_api_definition(value).map_err(bad_request)?
            }
        };

        self.definition_service
            .register(
                &internal_definition,
                CommonNamespace::default(),
                &EmptyAuthCtx {},
            )
            .await?;

        let definition = internal_definition.try_into().map_err(internal_error)?;

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
                CommonNamespace::default(),
                &EmptyAuthCtx {},
            )
            .await?
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
                CommonNamespace::default(),
                &EmptyAuthCtx {},
            )
            .await?;

        let definitions = definitions
            .into_iter()
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
            .get_all(CommonNamespace::default(), &EmptyAuthCtx {})
            .await?;

        let definitions = definitions
            .into_iter()
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

        let _ = self
            .definition_service
            .delete(
                &api_definition_id,
                &version,
                CommonNamespace::default(),
                &EmptyAuthCtx {},
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
