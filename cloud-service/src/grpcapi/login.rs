use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::grpcapi::get_authorisation_token;
use crate::model;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::login;
use crate::service::oauth2::{OAuth2Error, OAuth2Service};
use cloud_api_grpc::proto::golem::cloud::login::v1::cloud_login_service_server::CloudLoginService;
use cloud_api_grpc::proto::golem::cloud::login::v1::{
    complete_o_auth2_response, current_token_response, login_error, o_auth2_response,
    start_o_auth2_response, CompleteOAuth2Request, CompleteOAuth2Response, CurrentTokenRequest,
    CurrentTokenResponse, LoginError, OAuth2Request, OAuth2Response, StartOAuth2Response,
};
use cloud_api_grpc::proto::golem::cloud::login::OAuth2Data;
use cloud_api_grpc::proto::golem::cloud::token::{Token, UnsafeToken};
use golem_api_grpc::proto::golem::common::{Empty, ErrorBody, ErrorsBody};
use golem_common::metrics::api::TraceErrorKind;
use golem_common::recorded_grpc_api_request;
use golem_common::SafeDisplay;
use tonic::metadata::MetadataMap;
use tonic::{Request, Response, Status};
use tracing::Instrument;

impl From<AuthServiceError> for LoginError {
    fn from(value: AuthServiceError) -> Self {
        let error = match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. }
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. } => {
                login_error::Error::Internal(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => login_error::Error::Internal(ErrorBody {
                error: value.to_safe_string(),
            }),
        };
        LoginError { error: Some(error) }
    }
}

impl From<login::LoginError> for LoginError {
    fn from(value: login::LoginError) -> Self {
        let error = match value {
            login::LoginError::External(_) => login_error::Error::External(ErrorBody {
                error: value.to_safe_string(),
            }),
            login::LoginError::InternalAccountError(_)
            | login::LoginError::Internal(_)
            | login::LoginError::InternalOAuth2TokenError(_)
            | login::LoginError::InternalOAuth2ProviderClientError(_)
            | login::LoginError::InternalTokenServiceError(_) => {
                login_error::Error::Internal(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
        };
        LoginError { error: Some(error) }
    }
}

impl From<OAuth2Error> for LoginError {
    fn from(value: OAuth2Error) -> Self {
        let error = match value {
            OAuth2Error::InternalSessionError(_) | OAuth2Error::InternalGithubClientError(_) => {
                login_error::Error::Internal(ErrorBody {
                    error: value.to_safe_string(),
                })
            }
            OAuth2Error::InvalidSession(_) | OAuth2Error::InvalidState(_) => {
                login_error::Error::BadRequest(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                })
            }
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
        let record = recorded_grpc_api_request!("complete_login_oauth2",);

        let response = match self
            .complete_oauth2(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(complete_o_auth2_response::Result::Success(result)),
            Err(error) => record.fail(
                complete_o_auth2_response::Result::Error(error.clone()),
                &LoginTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CompleteOAuth2Response {
            result: Some(response),
        }))
    }

    async fn start_o_auth2(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<StartOAuth2Response>, Status> {
        let record = recorded_grpc_api_request!("start_login_oauth2",);

        let response = match self.start_oauth2().instrument(record.span.clone()).await {
            Ok(result) => record.succeed(start_o_auth2_response::Result::Success(result)),
            Err(error) => record.fail(
                start_o_auth2_response::Result::Error(error.clone()),
                &LoginTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(StartOAuth2Response {
            result: Some(response),
        }))
    }

    async fn current_token(
        &self,
        request: Request<CurrentTokenRequest>,
    ) -> Result<Response<CurrentTokenResponse>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!("current_login_token",);

        let response = match self
            .get_current_token(r, m)
            .instrument(record.span.clone())
            .await
        {
            Ok(result) => record.succeed(current_token_response::Result::Success(result)),
            Err(error) => record.fail(
                current_token_response::Result::Error(error.clone()),
                &LoginTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(CurrentTokenResponse {
            result: Some(response),
        }))
    }

    async fn o_auth2(
        &self,
        request: Request<OAuth2Request>,
    ) -> Result<Response<OAuth2Response>, Status> {
        let (m, _, r) = request.into_parts();

        let record = recorded_grpc_api_request!("login_oauth2",);

        let response = match self.oauth2(r, m).instrument(record.span.clone()).await {
            Ok(result) => record.succeed(o_auth2_response::Result::Success(result)),
            Err(error) => record.fail(
                o_auth2_response::Result::Error(error.clone()),
                &LoginTraceErrorKind(&error),
            ),
        };

        Ok(Response::new(OAuth2Response {
            result: Some(response),
        }))
    }
}

pub struct LoginTraceErrorKind<'a>(pub &'a LoginError);

impl Debug for LoginTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for LoginTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                login_error::Error::BadRequest(_) => "BadRequest",
                login_error::Error::Internal(_) => "Internal",
                login_error::Error::External(_) => "External",
            },
        }
    }

    fn is_expected(&self) -> bool {
        match &self.0.error {
            None => false,
            Some(error) => match error {
                login_error::Error::BadRequest(_) => true,
                login_error::Error::Internal(_) => true,
                login_error::Error::External(_) => false,
            },
        }
    }
}
