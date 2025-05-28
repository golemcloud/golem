use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::login::{LoginError as LoginServiceError, LoginService};
use crate::service::oauth2::{OAuth2Error, OAuth2Service};
use cloud_common::auth::GolemSecurityScheme;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::recorded_http_api_request;
use golem_common::SafeDisplay;
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

    fn is_expected(&self) -> bool {
        match &self {
            LoginError::BadRequest(_) => true,
            LoginError::External(_) => true,
            LoginError::Internal(_) => false,
        }
    }
}

impl LoginError {
    pub fn bad_request(error: impl Into<String>) -> Self {
        LoginError::BadRequest(Json(ErrorsBody {
            errors: vec![error.into()],
        }))
    }
}

type Result<T> = std::result::Result<T, LoginError>;

impl From<AuthServiceError> for LoginError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. }
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. } => {
                LoginError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => LoginError::Internal(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<LoginServiceError> for LoginError {
    fn from(value: LoginServiceError) -> Self {
        match value {
            LoginServiceError::External(_) => LoginError::External(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            LoginServiceError::InternalAccountError(_)
            | LoginServiceError::Internal(_)
            | LoginServiceError::InternalOAuth2ProviderClientError(_)
            | LoginServiceError::InternalOAuth2TokenError(_)
            | LoginServiceError::InternalTokenServiceError(_) => {
                LoginError::Internal(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}

impl From<OAuth2Error> for LoginError {
    fn from(value: OAuth2Error) -> Self {
        match value {
            OAuth2Error::InternalGithubClientError(_) | OAuth2Error::InternalSessionError(_) => {
                LoginError::Internal(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            OAuth2Error::InvalidSession(_) | OAuth2Error::InvalidState(_) => {
                LoginError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
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
    #[oai(path = "/v1/oauth2", method = "post", operation_id = "login_oauth2")]
    async fn oauth2(
        &self,
        /// Currently only `github` is supported.
        provider: Query<String>,
        /// OAuth2 access token
        #[oai(name = "access-token")]
        access_token: Query<String>,
    ) -> Result<Json<UnsafeToken>> {
        let record = recorded_http_api_request!("login_oauth2",);
        let response = self
            .oauth2_internal(provider.0, access_token.0)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn oauth2_internal(
        &self,
        provider: String,
        access_token: String,
    ) -> Result<Json<UnsafeToken>> {
        let auth_provider =
            OAuth2Provider::from_str(provider.as_str()).map_err(LoginError::bad_request)?;
        let result = self
            .login_service
            .oauth2(&auth_provider, &access_token)
            .await?;
        Ok(Json(result))
    }

    /// Get information about a token
    ///
    /// Gets information about a token that is selected by the secret key passed in the Authorization header.
    /// The JSON is the same as the data object in the oauth2 endpoint's response.
    #[oai(
        path = "/v1/login/token",
        method = "get",
        operation_id = "current_login_token"
    )]
    async fn current_token(&self, token: GolemSecurityScheme) -> Result<Json<Token>> {
        let record = recorded_http_api_request!("current_login_token",);
        let response = self
            .auth_service
            .authorization(token.as_ref())
            .instrument(record.span.clone())
            .await
            .map(|auth| Json(auth.token))
            .map_err(|err| err.into());

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
    async fn start_login_oauth2(&self) -> Result<Json<OAuth2Data>> {
        let record = recorded_http_api_request!("start_login_oauth2",);
        let response = self
            .oauth2_service
            .start_workflow()
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(|err| err.into());

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
    async fn complete_login_oauth2(&self, result: Json<String>) -> Result<Json<UnsafeToken>> {
        let record = recorded_http_api_request!("complete_login_oauth2",);
        let response = self
            .complete_login_oauth2_internal(result.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn complete_login_oauth2_internal(&self, result: String) -> Result<Json<UnsafeToken>> {
        let token = self
            .oauth2_service
            .finish_workflow(&EncodedOAuth2Session { value: result })
            .await?;
        let result = self
            .login_service
            .oauth2(&token.provider, &token.access_token)
            .await?;
        Ok(Json(result))
    }

    /// Initiate OAuth2 Web Flow
    ///
    /// Starts the OAuth2 web flow authorization process by returning the authorization URL for the given provider.
    #[oai(
        path = "/v1/login/oauth2/web/authorize",
        method = "get",
        operation_id = "oauth2_web_flow_start"
    )]
    async fn oauth2_web_flow_start(
        &self,
        /// Currently only `github` is supported.
        Query(provider): Query<String>,
        /// The redirect URL to redirect to after the user has authorized the application
        Query(redirect): Query<Option<String>>,
    ) -> Result<Json<WebFlowAuthorizeUrlResponse>> {
        let record = recorded_http_api_request!("oauth2_web_flow_start",);
        let response = self
            .oauth2_web_flow_start_internal(provider, redirect)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_web_flow_start_internal(
        &self,
        provider: String,
        redirect: Option<String>,
    ) -> Result<Json<WebFlowAuthorizeUrlResponse>> {
        let redirect = match redirect {
            Some(r) => {
                let url = url::Url::parse(&r)
                    .map_err(|_| LoginError::bad_request("Invalid redirect URL"))?;
                if url
                    .domain()
                    .is_some_and(|d| d.starts_with("localhost") || d.contains("golem.cloud"))
                {
                    Some(url)
                } else {
                    return Err(LoginError::bad_request("Invalid redirect domain"));
                }
            }
            None => None,
        };
        let provider =
            OAuth2Provider::from_str(provider.as_str()).map_err(LoginError::bad_request)?;
        let state = self
            .login_service
            .generate_temp_token_state(redirect)
            .await?;
        let url = self
            .oauth2_service
            .get_authorize_url(provider, &state)
            .await?;
        Ok(Json(WebFlowAuthorizeUrlResponse { url, state }))
    }

    /// GitHub OAuth2 Web Flow callback
    ///
    /// This endpoint handles the callback from GitHub after the user has authorized the application.
    /// It exchanges the code for an access token and then uses that to log the user in.
    #[oai(
        path = "/v1/login/oauth2/web/callback/github",
        method = "get",
        operation_id = "oauth2_web_flow_callback_github"
    )]
    async fn oauth2_web_flow_callback_github(
        &self,
        /// The authorization code returned by GitHub
        Query(code): Query<String>,
        /// The state parameter for CSRF protection
        Query(state): Query<String>,
    ) -> Result<WebFlowCallbackSuccess> {
        let record = recorded_http_api_request!("oauth2_web_flow_callback_github",);
        let response = self
            .oauth2_web_flow_callback_github_internal(code, state)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_web_flow_callback_github_internal(
        &self,
        code: String,
        state: String,
    ) -> Result<WebFlowCallbackSuccess> {
        // Exchange the code for an access token
        let access_token = self
            .oauth2_service
            .exchange_code_for_token(OAuth2Provider::Github, &code, &state)
            .await?;

        // Use the access token to log in or create a user
        let unsafe_token = self
            .login_service
            .oauth2(&OAuth2Provider::Github, &access_token)
            .await?;

        // Store the token temporarily
        let token = self
            .login_service
            .link_temp_token(&unsafe_token.data.id, &state)
            .await?;

        let response = if let Some(mut redirect) = token.metadata.redirect {
            redirect.query_pairs_mut().append_pair("state", &state);
            WebFlowCallbackSuccess::Redirect(
                Json(WebFlowCallbackSuccessResponse {}),
                redirect.to_string(),
            )
        } else {
            WebFlowCallbackSuccess::Success(Json(WebFlowCallbackSuccessResponse {}))
        };

        Ok(response)
    }

    /// Poll for OAuth2 Web Flow token
    ///
    /// This endpoint is used by clients to poll for the token after the user has authorized the application via the web flow.
    #[oai(
        path = "/v1/login/oauth2/web/poll",
        method = "get",
        operation_id = "oauth2_web_flow_poll"
    )]
    async fn oauth2_web_flow_poll(
        &self,
        /// The state parameter for identifying the session
        Query(state): Query<String>,
    ) -> Result<WebFlowPoll> {
        let record = recorded_http_api_request!("oauth2_web_flow_poll",);
        let response = self
            .oauth2_web_flow_poll_internal(state)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_web_flow_poll_internal(&self, state: String) -> Result<WebFlowPoll> {
        let token = self.login_service.get_temp_token(&state).await?;

        let result = match token {
            Some(token) => WebFlowPoll::Completed(Json(token.token)),
            None => WebFlowPoll::Pending(Json(PendingFlowCompletionResponse {})),
        };

        Ok(result)
    }
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowPoll {
    /// OAuth flow has completed
    #[oai(status = 200)]
    Completed(Json<UnsafeToken>),
    /// OAuth flow is pending
    #[oai(status = 202)]
    Pending(Json<PendingFlowCompletionResponse>),
}

#[derive(Debug, Clone, Object)]
pub struct PendingFlowCompletionResponse {}

#[derive(Debug, Clone, Object)]
pub struct WebFlowAuthorizeUrlResponse {
    pub url: String,
    pub state: String,
}

#[derive(Debug, Clone, ApiResponse)]
pub enum WebFlowCallbackSuccess {
    /// Redirect to the given URL specified in the web flow start
    #[oai(status = 302)]
    Redirect(
        Json<WebFlowCallbackSuccessResponse>,
        #[oai(header = "Location")] String,
    ),
    /// OAuth flow has completed
    #[oai(status = 200)]
    Success(Json<WebFlowCallbackSuccessResponse>),
}

#[derive(Debug, Clone, Object)]
pub struct WebFlowCallbackSuccessResponse {}
