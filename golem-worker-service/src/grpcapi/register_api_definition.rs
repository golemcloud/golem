use std::sync::Arc;

use async_trait::async_trait;

use golem_api_grpc::proto::golem::{
    apidefinition::{
        api_definition_registration_error, api_definition_service_server::ApiDefinitionService,
        create_or_update_open_api_response, create_or_update_response_api_definition_response,
        delete_api_definition_response, get_all_api_definition_versions_response,
        get_all_api_definitions_response, get_api_definition_response,
        ApiDefinitionRegistrationError, CreateOrUpdateApiDefinitionRequest,
        CreateOrUpdateOpenApiRequest, CreateOrUpdateOpenApiResponse,
        CreateOrUpdateResponseApiDefinitionResponse, DeleteApiDefinitionRequest,
        DeleteApiDefinitionResponse, GetAllApiDefinitionVersionsRequest,
        GetAllApiDefinitionVersionsResponse, GetAllApiDefinitionsRequest,
        GetAllApiDefinitionsResponse, GetApiDefinitionRequest, GetApiDefinitionResponse,
        HttpApiDefinition as GrpcHttpApiDefinition, HttpApiDefinitionList,
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

    async fn create_or_update_open_api_definition(
        &self,
        request: tonic::Request<CreateOrUpdateOpenApiRequest>,
    ) -> Result<tonic::Response<CreateOrUpdateOpenApiResponse>, tonic::Status> {
        let result = match self
            .create_or_update_open_api_definition(request.into_inner())
            .await
        {
            Ok(result) => create_or_update_open_api_response::Result::Success(result),
            Err(error) => create_or_update_open_api_response::Result::Error(error),
        };

        Ok(tonic::Response::new(CreateOrUpdateOpenApiResponse {
            result: Some(result),
        }))
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

    async fn get_all_api_definition_versions(
        &self,
        request: tonic::Request<GetAllApiDefinitionVersionsRequest>,
    ) -> Result<tonic::Response<GetAllApiDefinitionVersionsResponse>, tonic::Status> {
        let result =
            match self
                .get_all_api_definition_versions(request.into_inner())
                .await
            {
                Ok(api_definitions) => get_all_api_definition_versions_response::Result::Success(
                    HttpApiDefinitionList { api_definitions },
                ),
                Err(error) => get_all_api_definition_versions_response::Result::Error(error),
            };

        Ok(tonic::Response::new(GetAllApiDefinitionVersionsResponse {
            result: Some(result),
        }))
    }

    async fn get_all_api_definitions(
        &self,
        request: tonic::Request<GetAllApiDefinitionsRequest>,
    ) -> std::result::Result<tonic::Response<GetAllApiDefinitionsResponse>, tonic::Status> {
        let result = match self.get_all_api_definitions(request.into_inner()).await {
            Ok(api_definitions) => {
                get_all_api_definitions_response::Result::Success(HttpApiDefinitionList {
                    api_definitions,
                })
            }
            Err(error) => get_all_api_definitions_response::Result::Error(error),
        };

        Ok(tonic::Response::new(GetAllApiDefinitionsResponse {
            result: Some(result),
        }))
    }

    async fn delete(
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
    ) -> Result<GrpcHttpApiDefinition, ApiDefinitionRegistrationError> {
        let definition = request
            .payload
            .ok_or(bad_request("Missing Api Definition"))?;

        let internal_definition = definition.clone().try_into().map_err(bad_request)?;

        self.definition_service
            .register(
                &internal_definition,
                CommonNamespace::default(),
                &EmptyAuthCtx {},
            )
            .await?;

        Ok(definition)
    }

    async fn create_or_update_open_api_definition(
        &self,
        request: CreateOrUpdateOpenApiRequest,
    ) -> Result<GrpcHttpApiDefinition, ApiDefinitionRegistrationError> {
        let definition = request.payload;

        let value = serde_json::from_str(&definition).map_err(|_| bad_request("Invalid JSON"))?;

        let definition = get_api_definition(value).map_err(bad_request)?;

        self.definition_service
            .register(&definition, CommonNamespace::default(), &EmptyAuthCtx {})
            .await?;

        let grpc_definition = definition.try_into().map_err(internal_error)?;

        Ok(grpc_definition)
    }

    async fn get_api_definition(
        &self,
        request: GetApiDefinitionRequest,
    ) -> Result<GrpcHttpApiDefinition, ApiDefinitionRegistrationError> {
        let api_definition_id = ApiDefinitionId(request.api_definition_id);
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
        request: GetAllApiDefinitionVersionsRequest,
    ) -> Result<Vec<GrpcHttpApiDefinition>, ApiDefinitionRegistrationError> {
        let api_definition_id = ApiDefinitionId(request.api_definition_id);

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
            .collect::<Result<Vec<GrpcHttpApiDefinition>, _>>()
            .map_err(internal_error)?;

        Ok(definitions)
    }

    async fn get_all_api_definitions(
        &self,
        _request: GetAllApiDefinitionsRequest,
    ) -> Result<Vec<GrpcHttpApiDefinition>, ApiDefinitionRegistrationError> {
        let definitions = self
            .definition_service
            .get_all(CommonNamespace::default(), &EmptyAuthCtx {})
            .await?;

        let definitions = definitions
            .into_iter()
            .map(|d| d.try_into())
            .collect::<Result<Vec<GrpcHttpApiDefinition>, _>>()
            .map_err(internal_error)?;

        Ok(definitions)
    }

    async fn delete_api_definition(
        &self,
        request: DeleteApiDefinitionRequest,
    ) -> Result<(), ApiDefinitionRegistrationError> {
        let api_definition_id = ApiDefinitionId(request.api_definition_id);
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

fn bad_request(error: impl Into<String>) -> ApiDefinitionRegistrationError {
    ApiDefinitionRegistrationError {
        error: Some(api_definition_registration_error::Error::BadRequest(
            ErrorsBody {
                errors: vec![error.into()],
            },
        )),
    }
}

fn not_found(error: impl Into<String>) -> ApiDefinitionRegistrationError {
    ApiDefinitionRegistrationError {
        error: Some(api_definition_registration_error::Error::NotFound(
            ErrorBody {
                error: error.into(),
            },
        )),
    }
}

fn internal_error(error: impl Into<String>) -> ApiDefinitionRegistrationError {
    ApiDefinitionRegistrationError {
        error: Some(api_definition_registration_error::Error::InternalError(
            ErrorBody {
                error: error.into(),
            },
        )),
    }
}
