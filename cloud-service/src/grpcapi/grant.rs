use std::sync::Arc;

use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc_cloud::proto::golem::cloud::grant::cloud_grant_service_server::CloudGrantService;
use golem_api_grpc_cloud::proto::golem::cloud::grant::{
    delete_grant_response, get_grant_response, get_grants_response, put_grant_response,
    DeleteGrantRequest, DeleteGrantResponse, GetGrantRequest, GetGrantResponse, GetGrantsRequest,
    GetGrantsResponse, GetGrantsSuccessResponse, PutGrantRequest, PutGrantResponse,
};
use golem_api_grpc_cloud::proto::golem::cloud::grant::{grant_error, GrantError};
use golem_common::model::AccountId;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model::Role;
use crate::service::account_grant::{AccountGrantService, AccountGrantServiceError};
use crate::service::auth::{AuthService, AuthServiceError};

impl From<AuthServiceError> for GrantError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                grant_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                grant_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        GrantError { error: Some(error) }
    }
}

impl From<AccountGrantServiceError> for GrantError {
    fn from(value: AccountGrantServiceError) -> Self {
        let error = match value {
            AccountGrantServiceError::Unauthorized(error) => {
                grant_error::Error::Unauthorized(ErrorBody { error })
            }
            AccountGrantServiceError::Unexpected(error) => {
                grant_error::Error::InternalError(ErrorBody { error })
            }
            AccountGrantServiceError::ArgValidation(errors) => {
                grant_error::Error::BadRequest(ErrorsBody { errors })
            }
        };
        GrantError { error: Some(error) }
    }
}

fn bad_request_error(error: &str) -> GrantError {
    GrantError {
        error: Some(grant_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

pub struct GrantGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_grant_service: Arc<dyn AccountGrantService + Sync + Send>,
}

impl GrantGrpcApi {
    async fn auth(&self, metadata: MetadataMap) -> Result<AccountAuthorisation, GrantError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(GrantError {
                error: Some(grant_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn get_by_account(
        &self,
        request: GetGrantsRequest,
        metadata: MetadataMap,
    ) -> Result<GetGrantsSuccessResponse, GrantError> {
        let auth = self.auth(metadata).await?;

        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        let values = self.account_grant_service.get(&account_id, &auth).await?;

        let roles = values
            .iter()
            .map(|a| a.clone().into())
            .collect::<Vec<i32>>();

        Ok(GetGrantsSuccessResponse { roles })
    }

    async fn get(
        &self,
        request: GetGrantRequest,
        metadata: MetadataMap,
    ) -> Result<i32, GrantError> {
        let auth = self.auth(metadata).await?;

        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        let role: Role = request
            .role
            .try_into()
            .map_err(|_| bad_request_error("Invalid role"))?;

        let values = self.account_grant_service.get(&account_id, &auth).await?;

        if values.contains(&role) {
            Ok(request.role)
        } else {
            Err(GrantError {
                error: Some(grant_error::Error::NotFound(ErrorBody {
                    error: "Role not found".to_string(),
                })),
            })
        }
    }

    async fn delete(
        &self,
        request: DeleteGrantRequest,
        metadata: MetadataMap,
    ) -> Result<(), GrantError> {
        let auth = self.auth(metadata).await?;

        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        let role: Role = request
            .role
            .try_into()
            .map_err(|_| bad_request_error("Invalid role"))?;

        self.account_grant_service
            .remove(&account_id, &role, &auth)
            .await?;
        Ok(())
    }

    async fn put(
        &self,
        request: PutGrantRequest,
        metadata: MetadataMap,
    ) -> Result<i32, GrantError> {
        let auth = self.auth(metadata).await?;

        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        let role: Role = request
            .role
            .try_into()
            .map_err(|_| bad_request_error("Invalid role"))?;

        self.account_grant_service
            .add(&account_id, &role, &auth)
            .await?;
        Ok(request.role)
    }
}

#[async_trait::async_trait]
impl CloudGrantService for GrantGrpcApi {
    async fn get_grants(
        &self,
        request: Request<GetGrantsRequest>,
    ) -> Result<Response<GetGrantsResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get_by_account(r, m).await {
            Ok(result) => Ok(Response::new(GetGrantsResponse {
                result: Some(get_grants_response::Result::Success(result)),
            })),
            Err(err) => Ok(Response::new(GetGrantsResponse {
                result: Some(get_grants_response::Result::Error(err)),
            })),
        }
    }

    async fn delete_grant(
        &self,
        request: Request<DeleteGrantRequest>,
    ) -> Result<Response<DeleteGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.delete(r, m).await {
            Ok(_) => Ok(Response::new(DeleteGrantResponse {
                result: Some(delete_grant_response::Result::Success(Empty {})),
            })),
            Err(err) => Ok(Response::new(DeleteGrantResponse {
                result: Some(delete_grant_response::Result::Error(err)),
            })),
        }
    }

    async fn get_grant(
        &self,
        request: Request<GetGrantRequest>,
    ) -> Result<Response<GetGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get(r, m).await {
            Ok(result) => Ok(Response::new(GetGrantResponse {
                result: Some(get_grant_response::Result::Role(result)),
            })),
            Err(err) => Ok(Response::new(GetGrantResponse {
                result: Some(get_grant_response::Result::Error(err)),
            })),
        }
    }

    async fn put_grant(
        &self,
        request: Request<PutGrantRequest>,
    ) -> Result<Response<PutGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.put(r, m).await {
            Ok(result) => Ok(Response::new(PutGrantResponse {
                result: Some(put_grant_response::Result::Role(result)),
            })),
            Err(err) => Ok(Response::new(PutGrantResponse {
                result: Some(put_grant_response::Result::Error(err)),
            })),
        }
    }
}
