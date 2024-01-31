use std::str::FromStr;
use std::sync::Arc;

use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_api_grpc_cloud::proto::golem::cloud::login::cloud_login_service_server::CloudLoginService;
use golem_api_grpc_cloud::proto::golem::cloud::login::{
    complete_o_auth2_response, current_token_response, o_auth2_response, start_o_auth2_response,
    CompleteOAuth2Request, CompleteOAuth2Response, CurrentTokenRequest, CurrentTokenResponse,
    OAuth2Request, OAuth2Response, StartOAuth2Response,
};
use golem_api_grpc_cloud::proto::golem::cloud::login::{login_error, LoginError, OAuth2Data};
use golem_api_grpc_cloud::proto::golem::cloud::token::{Token, UnsafeToken};
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::login;
use crate::service::oauth2::{OAuth2Error, OAuth2Service};

impl From<AuthServiceError> for LoginError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(error) => login_error::Error::BadRequest(ErrorsBody {
                errors: vec![error],
            }),
            AuthServiceError::Unexpected(error) => {
                login_error::Error::Internal(ErrorBody { error })
            }
        };
        LoginError { error: Some(error) }
    }
}

impl From<login::LoginError> for LoginError {
    fn from(value: login::LoginError) -> Self {
        let error = match value {
            login::LoginError::External(error) => login_error::Error::External(ErrorBody { error }),
            login::LoginError::Unexpected(error) => {
                login_error::Error::Internal(ErrorBody { error })
            }
        };
        LoginError { error: Some(error) }
    }
}

impl From<OAuth2Error> for LoginError {
    fn from(value: OAuth2Error) -> Self {
        let error = match value {
            OAuth2Error::Unexpected(error) => login_error::Error::Internal(ErrorBody { error }),
            OAuth2Error::InvalidSession(error) => login_error::Error::BadRequest(ErrorsBody {
                errors: vec![error],
            }),
        };
        LoginError { error: Some(error) }
    }
}

pub struct LoginGrpcApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub login_service: Arc<dyn login::LoginService + Sync + Send>,
    pub oauth2_service: Arc<dyn OAuth2Service + Sync + Send>,
}

impl LoginGrpcApi {
    async fn auth(&self, metadata: MetadataMap) -> Result<AccountAuthorisation, LoginError> {
        match get_authorisation_token(metadata) {
            Some(t) => self
                .auth_service
                .authorization(&t)
                .await
                .map_err(|e| e.into()),
            None => Err(LoginError {
                error: Some(login_error::Error::BadRequest(ErrorsBody {
                    errors: vec!["Missing token".into()],
                })),
            }),
        }
    }

    async fn get_current_token(
        &self,
        _request: CurrentTokenRequest,
        metadata: MetadataMap,
    ) -> Result<Token, LoginError> {
        let auth = self.auth(metadata).await?;
        Ok(auth.token.into())
    }

    async fn oauth2(
        &self,
        request: OAuth2Request,
        _metadata: MetadataMap,
    ) -> Result<UnsafeToken, LoginError> {
        let provider: model::OAuth2Provider =
            model::OAuth2Provider::from_str(request.provider.as_str()).map_err(|_| LoginError {
                error: Some(login_error::Error::BadRequest(ErrorsBody {
                    errors: vec!["Invalid provider".into()],
                })),
            })?;

        let result = self
            .login_service
            .oauth2(&provider, &request.access_token)
            .await?;
        Ok(result.into())
    }

    async fn complete_oauth2(
        &self,
        request: CompleteOAuth2Request,
        _metadata: MetadataMap,
    ) -> Result<UnsafeToken, LoginError> {
        let token = self
            .oauth2_service
            .finish_workflow(&model::EncodedOAuth2Session {
                value: request.body,
            })
            .await?;
        let result = self
            .login_service
            .oauth2(&token.provider, &token.access_token)
            .await?;

        Ok(result.into())
    }

    async fn start_oauth2(&self) -> Result<OAuth2Data, LoginError> {
        let result = self.oauth2_service.start_workflow().await?;
        Ok(result.into())
    }
}

#[async_trait::async_trait]
impl CloudLoginService for LoginGrpcApi {
    async fn complete_o_auth2(
        &self,
        request: Request<CompleteOAuth2Request>,
    ) -> Result<Response<CompleteOAuth2Response>, Status> {
        let (m, _, r) = request.into_parts();
        match self.complete_oauth2(r, m).await {
            Ok(v) => Ok(Response::new(CompleteOAuth2Response {
                result: Some(complete_o_auth2_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(CompleteOAuth2Response {
                result: Some(complete_o_auth2_response::Result::Error(err)),
            })),
        }
    }

    async fn start_o_auth2(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<StartOAuth2Response>, Status> {
        match self.start_oauth2().await {
            Ok(v) => Ok(Response::new(StartOAuth2Response {
                result: Some(start_o_auth2_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(StartOAuth2Response {
                result: Some(start_o_auth2_response::Result::Error(err)),
            })),
        }
    }

    async fn current_token(
        &self,
        request: Request<CurrentTokenRequest>,
    ) -> Result<Response<CurrentTokenResponse>, Status> {
        let (m, _, r) = request.into_parts();
        match self.get_current_token(r, m).await {
            Ok(v) => Ok(Response::new(CurrentTokenResponse {
                result: Some(current_token_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(CurrentTokenResponse {
                result: Some(current_token_response::Result::Error(err)),
            })),
        }
    }

    async fn o_auth2(
        &self,
        request: Request<OAuth2Request>,
    ) -> Result<Response<OAuth2Response>, Status> {
        let (m, _, r) = request.into_parts();
        match self.oauth2(r, m).await {
            Ok(v) => Ok(Response::new(OAuth2Response {
                result: Some(o_auth2_response::Result::Success(v)),
            })),
            Err(err) => Ok(Response::new(OAuth2Response {
                result: Some(o_auth2_response::Result::Error(err)),
            })),
        }
    }
}
