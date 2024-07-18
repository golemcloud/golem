use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::token;
use cloud_api_grpc::proto::golem::cloud::token::cloud_token_service_server::CloudTokenService;
use cloud_api_grpc::proto::golem::cloud::token::{
    create_token_response, delete_token_response, get_token_response, get_tokens_response,
    token_error, CreateTokenRequest, CreateTokenResponse, DeleteTokenRequest, DeleteTokenResponse,
    GetTokenRequest, GetTokenResponse, GetTokensRequest, GetTokensResponse,
    GetTokensSuccessResponse, Token, TokenError, UnsafeToken,
};
use cloud_common::grpc::proto_token_id_string;
use cloud_common::model::TokenId;
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_common::grpc::proto_account_id_string;
use golem_common::metrics::grpc::TraceErrorKind;
use golem_common::model::AccountId;
use golem_common::recorded_grpc_request;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

impl From<AuthServiceError> for TokenError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => {
                token_error::Error::Unauthorized(ErrorBody { error })
            }
            AuthServiceError::Unexpected(error) => {
                token_error::Error::Unauthorized(ErrorBody { error })
            }
        };
        TokenError { error: Some(error) }
    }
}

impl From<token::TokenServiceError> for TokenError {
    fn from(value: token::TokenServiceError) -> Self {
        let error = match value {
            token::TokenServiceError::Unauthorized(error) => {
                token_error::Error::Unauthorized(ErrorBody { error })
            }
            token::TokenServiceError::Unexpected(error) => {
                token_error::Error::InternalError(ErrorBody { error })
            }
            token::TokenServiceError::UnknownTokenId(_) => {
                token_error::Error::NotFound(ErrorBody {
                    error: "Token not found".to_string(),
                })
            }
            token::TokenServiceError::ArgValidation(errors) => {
                token_error::Error::BadRequest(ErrorsBody { errors })
            }
        };
        TokenError { error: Some(error) }
    }
}

fn bad_request_error(error: &str) -> TokenError {
    TokenError {
        error: Some(token_error::Error::BadRequest(ErrorsBody {
            errors: vec![error.to_string()],
        })),
    }
}

pub struct TokenGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub token_service: Arc<dyn token::TokenService + Sync + Send>,
}

impl TokenGrpcApi {
    async fn auth(&self, metadata: MetadataMap) -> Result<AccountAuthorisation, TokenError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(TokenError {
                error: Some(token_error::Error::Unauthorized(ErrorBody {
                    error: "Missing token".into(),
                })),
            }),
        }
    }

    async fn delete(
        &self,
        request: DeleteTokenRequest,
        metadata: MetadataMap,
    ) -> Result<(), TokenError> {
        let auth = self.auth(metadata).await?;
        let id: TokenId = request
            .token_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing token id"))?;

        self.token_service.delete(&id, &auth).await?;

        Ok(())
    }

    async fn create(
        &self,
        request: CreateTokenRequest,
        metadata: MetadataMap,
    ) -> Result<UnsafeToken, TokenError> {
        let auth = self.auth(metadata).await?;
        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;
        let expires_at: chrono::DateTime<chrono::Utc> = request
            .create_token_dto
            .and_then(|d| chrono::DateTime::<chrono::Utc>::from_str(d.expires_at.as_str()).ok())
            .ok_or_else(|| bad_request_error("Missing expires at"))?;
        let result = self
            .token_service
            .create(&account_id, &expires_at, &auth)
            .await?;
        Ok(result.into())
    }

    async fn get(
        &self,
        request: GetTokenRequest,
        metadata: MetadataMap,
    ) -> Result<Token, TokenError> {
        let auth = self.auth(metadata).await?;
        let id: TokenId = request
            .token_id
            .and_then(|id| id.try_into().ok())
            .ok_or_else(|| bad_request_error("Missing token id"))?;
        let result = self.token_service.get(&id, &auth).await?;
        Ok(result.into())
    }

    async fn get_by_account(
        &self,
        request: GetTokensRequest,
        metadata: MetadataMap,
    ) -> Result<Vec<Token>, TokenError> {
        let auth = self.auth(metadata).await?;
        let account_id: AccountId = request
            .account_id
            .map(|id| id.into())
            .ok_or_else(|| bad_request_error("Missing account id"))?;

        let result = self.token_service.find(&account_id, &auth).await?;
        Ok(result.into_iter().map(|p| p.into()).collect())
    }
}

#[async_trait::async_trait]
impl CloudTokenService for TokenGrpcApi {
    async fn get_tokens(
        &self,
        request: Request<GetTokensRequest>,
    ) -> Result<Response<GetTokensResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_request!(
            "get_tokens",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self
            .get_by_account(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(data) => record.succeed(get_tokens_response::Result::Success(
                GetTokensSuccessResponse { data },
            )),
            Err(error) => record.fail(
                get_tokens_response::Result::Error(error.clone()),
                &TokenTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetTokensResponse {
            result: Some(response),
        }))
    }

    async fn create_token(
        &self,
        request: Request<CreateTokenRequest>,
    ) -> Result<Response<CreateTokenResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_request!(
            "create_token",
            account_id = proto_account_id_string(&r.account_id)
        );

        let response = match self.create(r, m).instrument(record.span.clone()).await {
            Ok(data) => record.succeed(create_token_response::Result::Success(data)),
            Err(error) => record.fail(
                create_token_response::Result::Error(error.clone()),
                &TokenTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CreateTokenResponse {
            result: Some(response),
        }))
    }

    async fn delete_token(
        &self,
        request: Request<DeleteTokenRequest>,
    ) -> Result<Response<DeleteTokenResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_request!(
            "delete_token",
            account_id = proto_account_id_string(&r.account_id),
            token_Id = proto_token_id_string(&r.token_id)
        );

        let response = match self.delete(r, m).instrument(record.span.clone()).await {
            Ok(_) => record.succeed(delete_token_response::Result::Success(Empty {})),
            Err(error) => record.fail(
                delete_token_response::Result::Error(error.clone()),
                &TokenTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(DeleteTokenResponse {
            result: Some(response),
        }))
    }

    async fn get_token(
        &self,
        request: Request<GetTokenRequest>,
    ) -> Result<Response<GetTokenResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_request!(
            "get_token",
            account_id = proto_account_id_string(&r.account_id),
            token_Id = proto_token_id_string(&r.token_id)
        );

        let response = match self.get(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(get_token_response::Result::Success(result)),
            Err(error) => record.fail(
                get_token_response::Result::Error(error.clone()),
                &TokenTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(GetTokenResponse {
            result: Some(response),
        }))
    }
}

pub struct TokenTraceErrorKind<'a>(pub &'a TokenError);

impl<'a> Debug for TokenTraceErrorKind<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'a> TraceErrorKind for TokenTraceErrorKind<'a> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                token_error::Error::BadRequest(_) => "BadRequest",
                token_error::Error::Unauthorized(_) => "Unauthorized",
                token_error::Error::NotFound(_) => "NotFound",
                token_error::Error::InternalError(_) => "InternalError",
            },
        }
    }
}
