use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::login::{LoginError as LoginServiceError, LoginService};
use crate::service::oauth2::{OAuth2Error, OAuth2Service};
use cloud_common::auth::GolemSecurityScheme;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::recorded_http_api_request;
use golem_service_base::model::{ErrorBody, ErrorsBody};
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::str::FromStr;
use std::sync::Arc;
use tracing::Instrument;

#[derive(ApiResponse, Debug, Clone)]
pub enum LoginError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Failed to login
    #[oai(status = 401)]
    External(Json<ErrorBody>),
    /// External service call failed during login
    #[oai(status = 500)]
    Internal(Json<ErrorBody>),
}

impl TraceErrorKind for LoginError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            LoginError::BadRequest(_) => "BadRequest",
            LoginError::External(_) => "External",
            LoginError::Internal(_) => "Internal",
        }
    }
}

impl LoginError {
    pub fn bad_request(error: String) -> Self {
        LoginError::BadRequest(Json(ErrorsBody {
            errors: vec![error],
        }))
    }
}

type Result<T> = std::result::Result<T, LoginError>;

impl From<AuthServiceError> for LoginError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => LoginError::BadRequest(Json(ErrorsBody {
                errors: vec![error],
            })),
            AuthServiceError::Unexpected(error) => LoginError::Internal(Json(ErrorBody { error })),
        }
    }
}

impl From<LoginServiceError> for LoginError {
    fn from(value: LoginServiceError) -> Self {
        match value {
            LoginServiceError::Unexpected(error) => LoginError::Internal(Json(ErrorBody { error })),
            LoginServiceError::External(error) => LoginError::External(Json(ErrorBody { error })),
        }
    }
}

impl From<OAuth2Error> for LoginError {
    fn from(value: OAuth2Error) -> Self {
        match value {
            OAuth2Error::Unexpected(error) => LoginError::Internal(Json(ErrorBody { error })),
            OAuth2Error::InvalidSession(error) => LoginError::BadRequest(Json(ErrorsBody {
                errors: vec![error],
            })),
        }
    }
}

pub struct LoginApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub login_service: Arc<dyn LoginService + Sync + Send>,
    pub oauth2_service: Arc<dyn OAuth2Service + Sync + Send>,
}

#[OpenApi(prefix_path = "", tag = ApiTags::Login)]
impl LoginApi {
    /// Acquire token with OAuth2 authorization
    ///
    /// Gets a token by authorizing with an external OAuth2 provider. Currently only github is supported.
    ///
    /// In the response:
    /// - `id` is the identifier of the token itself
    /// - `accountId` is the account's identifier, can be used on the account API
    /// - `secret` is the secret key to be sent in the Authorization header as a bearer token for all the other endpoints
    ///
    #[oai(path = "/v2/oauth2", method = "post", operation_id = "login_oauth2")]
    async fn oauth2(
        &self,
        /// Currently only `github` is supported.
        provider: Query<String>,
        /// OAuth2 access token
        #[oai(name = "access-token")]
        access_token: Query<String>,
    ) -> Result<Json<UnsafeToken>> {
        let record = recorded_http_api_request!("login_oauth2",);
        let response = {
            let auth_provider =
                OAuth2Provider::from_str(provider.0.as_str()).map_err(LoginError::bad_request)?;
            let result = self
                .login_service
                .oauth2(&auth_provider, &access_token.0)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(result))
        };

        record.result(response)
    }

    /// Get information about a token
    ///
    /// Gets information about a token that is selected by the secret key passed in the Authorization header.
    /// The JSON is the same as the data object in the oauth2 endpoint's response.
    #[oai(
        path = "/v2/login/token",
        method = "get",
        operation_id = "current_login_token"
    )]
    async fn current_token(&self, token: GolemSecurityScheme) -> Result<Json<crate::model::Token>> {
        let record = recorded_http_api_request!("current_login_token",);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            Ok(Json(auth.token))
        };

        record.result(response)
    }

    /// Start GitHub OAuth2 interactive flow
    ///
    /// Starts an interactive authorization flow.
    /// The user must open the returned url and enter the userCode in a form before the expires deadline.
    /// Then the finish GitHub OAuth2 interactive flow endpoint must be called with the encoded session to finish the flow.
    #[oai(
        path = "/login/oauth2/device/start",
        method = "post",
        operation_id = "start_login_oauth2"
    )]
    async fn start_o_auth2(&self) -> Result<Json<OAuth2Data>> {
        let record = recorded_http_api_request!("start_login_oauth2",);
        let response = {
            let result = self
                .oauth2_service
                .start_workflow()
                .instrument(record.span.clone())
                .await?;
            Ok(Json(result))
        };

        record.result(response)
    }

    /// Finish GitHub OAuth2 interactive flow
    ///
    /// Finishes an interactive authorization flow. The returned JSON is equivalent to the oauth2 endpoint's response.
    /// Returns a JSON string containing the encodedSession from the start endpoint's response.
    #[oai(
        path = "/login/oauth2/device/complete",
        method = "post",
        operation_id = "complete_login_oauth2"
    )]
    async fn complete_o_auth2(&self, result: Json<String>) -> Result<Json<UnsafeToken>> {
        let record = recorded_http_api_request!("complete_login_oauth2",);
        let response = {
            let token = self
                .oauth2_service
                .finish_workflow(&EncodedOAuth2Session { value: result.0 })
                .instrument(record.span.clone())
                .await?;
            let result = self
                .login_service
                .oauth2(&token.provider, &token.access_token)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(result))
        };

        record.result(response)
    }
}
