use std::fmt::{Debug, Formatter};
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::service::account_grant::{AccountGrantService, AccountGrantServiceError};
use crate::service::auth::{AuthService, AuthServiceError};
use cloud_api_grpc::proto::golem::cloud::grant::v1::cloud_grant_service_server::CloudGrantService;
use cloud_api_grpc::proto::golem::cloud::grant::v1::{
    delete_grant_response, get_grant_response, get_grants_response, grant_error,
    put_grant_response, DeleteGrantRequest, DeleteGrantResponse, GetGrantRequest, GetGrantResponse,
    GetGrantsRequest, GetGrantsResponse, GetGrantsSuccessResponse, GrantError, PutGrantRequest,
    PutGrantResponse,
};
use cloud_common::model::Role;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_common::grpc::proto_account_id_string;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::AccountId;
use golem_common::recorded_grpc_api_request;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

impl From<AuthServiceError> for GrantError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(_) => grant_error::Error::Unauthorized(ErrorBody {
                error: value.to_string(),
            }),
            AuthServiceError::Internal(_) => grant_error::Error::Unauthorized(ErrorBody {
                error: value.to_string(),
            }),
        };
        GrantError { error: Some(error) }
    }
}

impl From<AccountGrantServiceError> for GrantError {
    fn from(value: AccountGrantServiceError) -> Self {
        let error = match value {
            AccountGrantServiceError::Unauthorized(_) => {
                grant_error::Error::Unauthorized(ErrorBody {
                    error: value.to_string(),
                })
            }
            AccountGrantServiceError::Internal(_) => grant_error::Error::InternalError(ErrorBody {
                error: value.to_string(),
            }),
            AccountGrantServiceError::ArgValidation(errors) => {
                grant_error::Error::BadRequest(ErrorsBody { errors })
            }
            AccountGrantServiceError::AccountNotFound(_) => {
                grant_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_string()],
                })
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

        let record = recorded_grpc_api_request!(
            "get_account_grants",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self
            .get_by_account(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(get_grants_response::Result::Success(result)),
            Err(error) => record.fail(
                get_grants_response::Result::Error(error.clone()),
                &GrantTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetGrantsResponse {
            result: Some(response),
        }))
    }

    async fn delete_grant(
        &self,
        request: Request<DeleteGrantRequest>,
    ) -> Result<Response<DeleteGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "delete_account_grant",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.delete(r, m).instrument(record.span.clone()).await {
            Ok(_) => record.succeed(delete_grant_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                delete_grant_response::Result::Error(error.clone()),
                &GrantTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(DeleteGrantResponse {
            result: Some(response),
        }))
    }

    async fn get_grant(
        &self,
        request: Request<GetGrantRequest>,
    ) -> Result<Response<GetGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "get_account_grant",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_grant_response::Result::Role(result)),
            Err(error) => record.fail(
                get_grant_response::Result::Error(error.clone()),
                &GrantTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetGrantResponse {
            result: Some(response),
        }))
    }

    async fn put_grant(
        &self,
        request: Request<PutGrantRequest>,
    ) -> Result<Response<PutGrantResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!(
            "create_account_grant",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.put(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(put_grant_response::Result::Role(result)),
            Err(error) => record.fail(
                put_grant_response::Result::Error(error.clone()),
                &GrantTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(PutGrantResponse {
            result: Some(response),
        }))
    }
}

pub struct GrantTraceErrorKind<'a>(pub &'a GrantError);

impl<'a> Debug for GrantTraceErrorKind<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'a> TraceErrorKind for GrantTraceErrorKind<'a> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                grant_error::Error::BadRequest(_) => "BadRequest",
                grant_error::Error::Unauthorized(_) => "Unauthorized",
                grant_error::Error::NotFound(_) => "NotFound",
                grant_error::Error::InternalError(_) => "InternalError",
            },
        }
    }
}
