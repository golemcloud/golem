// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::ApiResult;
use super::error::ApiError;
use super::model::{WebFlowCallbackSuccess, WebFlowPollResponse};
use crate::bootstrap::login::{LoginSystem, LoginSystemEnabled};
use golem_common::model::Empty;
use golem_common::model::auth::{Token, TokenWithSecret};
use golem_common::model::error::ErrorBody;
use golem_common::model::login::{
    EncodedOAuth2Session, OAuth2DeviceFlowData, OAuth2DeviceFlowStartRequest, OAuth2Provider,
    OAuth2WebWorkflowData, OAuth2WebflowStateId,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct LoginApi {
    pub login_system: LoginSystem,
}

#[OpenApi(
    prefix_path = "/v1/login",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Login
)]
impl LoginApi {
    pub fn new(login_system: LoginSystem) -> Self {
        Self { login_system }
    }

    /// Get information about a token
    ///
    /// Gets information about a token that is selected by the secret key passed in the Authorization header.
    /// The JSON is the same as the data object in the oauth2 endpoint's response.
    #[oai(path = "/token", method = "get", operation_id = "current_login_token")]
    async fn current_token(&self, token: GolemSecurityScheme) -> ApiResult<Json<Token>> {
        let record = recorded_http_api_request!("current_login_token",);
        let response = self
            .current_token_internal(token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn current_token_internal(&self, _token: GolemSecurityScheme) -> ApiResult<Json<Token>> {
        todo!()
    }

    /// Start OAuth2 interactive flow
    ///
    /// Starts an interactive authorization flow.
    /// The user must open the returned url and enter the userCode in a form before the expires deadline.
    /// Then the finish GitHub OAuth2 interactive flow endpoint must be called with the encoded session to finish the flow.
    #[oai(
        path = "/oauth2/device/start",
        method = "post",
        operation_id = "start_login_oauth2"
    )]
    async fn start_oauth2_device_flow(
        &self,
        request: Json<OAuth2DeviceFlowStartRequest>,
    ) -> ApiResult<Json<OAuth2DeviceFlowData>> {
        let record = recorded_http_api_request!("start_oauth2_device_flow",);

        let response = self
            .start_oauth2_device_flow_internal(request.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn start_oauth2_device_flow_internal(
        &self,
        request: OAuth2DeviceFlowStartRequest,
    ) -> ApiResult<Json<OAuth2DeviceFlowData>> {
        let login_system = self.get_enabled_login_system()?;

        let result = login_system
            .oauth2_service
            .start_device_workflow(request.provider)
            .await
            .map(Json)?;

        Ok(result)
    }

    /// Finish GitHub OAuth2 interactive flow
    ///
    /// Finishes an interactive authorization flow. The returned JSON is equivalent to the oauth2 endpoint's response.
    /// Returns a JSON string containing the encodedSession from the start endpoint's response.
    #[oai(
        path = "/oauth2/device/complete",
        method = "post",
        operation_id = "complete_login_oauth2"
    )]
    async fn complete_oauth2_device_flow(
        &self,
        result: Json<EncodedOAuth2Session>,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let record = recorded_http_api_request!("complete_oauth2_device_flow",);
        let response = self
            .complete_oauth2_device_flow_internal(result.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn complete_oauth2_device_flow_internal(
        &self,
        result: EncodedOAuth2Session,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let login_system = self.get_enabled_login_system()?;

        let result = login_system
            .oauth2_service
            .finish_device_workflow(&result)
            .await?;

        Ok(Json(result))
    }

    /// Initiate OAuth2 Web Flow
    ///
    /// Starts the OAuth2 web flow authorization process by returning the authorization URL for the given provider.
    #[oai(
        path = "/oauth2/web/authorize",
        method = "get",
        operation_id = "oauth2_web_flow_start"
    )]
    async fn oauth2_web_flow_start(
        &self,
        /// Currently only `github` is supported.
        Query(provider): Query<OAuth2Provider>,
        /// The redirect URL to redirect to after the user has authorized the application
        Query(redirect): Query<Option<String>>,
    ) -> ApiResult<Json<OAuth2WebWorkflowData>> {
        let record = recorded_http_api_request!("oauth2_web_flow_start",);
        let response = self
            .oauth2_web_flow_start_internal(provider, redirect)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_web_flow_start_internal(
        &self,
        provider: OAuth2Provider,
        redirect: Option<String>,
    ) -> ApiResult<Json<OAuth2WebWorkflowData>> {
        let login_system = self.get_enabled_login_system()?;

        let redirect = match redirect {
            Some(r) => {
                let url = url::Url::parse(&r)
                    .map_err(|_| ApiError::bad_request("Invalid redirect URL".to_string()))?;
                if url
                    .domain()
                    .is_some_and(|d| d.starts_with("localhost") || d.contains("golem.cloud"))
                {
                    Some(url)
                } else {
                    return Err(ApiError::bad_request("Invalid redirect domain".to_string()));
                }
            }
            None => None,
        };

        let result = login_system
            .oauth2_service
            .start_web_workflow(&provider, redirect)
            .await?;

        Ok(Json(result))
    }

    /// GitHub OAuth2 Web Flow callback
    ///
    /// This endpoint handles the callback from GitHub after the user has authorized the application.
    /// It exchanges the code for an access token and then uses that to log the user in.
    #[oai(
        path = "/oauth2/web/callback",
        method = "get",
        operation_id = "oauth2_web_flow_callback_github"
    )]
    async fn oauth2_web_flow_callback_github(
        &self,
        /// The authorization code returned by GitHub
        Query(code): Query<String>,
        /// The state parameter for CSRF protection
        Query(state): Query<OAuth2WebflowStateId>,
    ) -> ApiResult<WebFlowCallbackSuccess> {
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
        state: OAuth2WebflowStateId,
    ) -> ApiResult<WebFlowCallbackSuccess> {
        let login_system = self.get_enabled_login_system()?;

        let state_metadata = login_system
            .oauth2_service
            .handle_web_workflow_callback(&state, code)
            .await?;

        let response = if let Some(mut redirect) = state_metadata.redirect {
            redirect
                .query_pairs_mut()
                .append_pair("state", &state.0.to_string());
            WebFlowCallbackSuccess::Redirect(Json(Empty {}), redirect.to_string())
        } else {
            WebFlowCallbackSuccess::Success(Json(Empty {}))
        };

        Ok(response)
    }

    /// Poll for OAuth2 Web Flow token
    ///
    /// This endpoint is used by clients to poll for the token after the user has authorized the application via the web flow.
    #[oai(
        path = "/oauth2/web/poll",
        method = "get",
        operation_id = "oauth2_web_flow_poll"
    )]
    async fn oauth2_web_flow_poll(
        &self,
        /// The state parameter for identifying the session
        Query(state): Query<OAuth2WebflowStateId>,
    ) -> ApiResult<WebFlowPollResponse> {
        let record = recorded_http_api_request!("oauth2_web_flow_poll",);
        let response = self
            .oauth2_web_flow_poll_internal(state)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_web_flow_poll_internal(
        &self,
        state_id: OAuth2WebflowStateId,
    ) -> ApiResult<WebFlowPollResponse> {
        let login_system = self.get_enabled_login_system()?;

        let state = login_system
            .oauth2_service
            .get_web_workflow_state(&state_id)
            .await?;

        let response = match state.token {
            Some(token) => WebFlowPollResponse::Completed(Json(token)),
            None => WebFlowPollResponse::Pending(Json(Empty {})),
        };

        Ok(response)
    }

    fn get_enabled_login_system(&self) -> ApiResult<&LoginSystemEnabled> {
        match &self.login_system {
            LoginSystem::Enabled(inner) => Ok(inner),
            LoginSystem::Disabled => Err(ApiError::Conflict(Json(ErrorBody {
                error: "Logins are disabled by configuration".to_string(),
                cause: None,
            }))),
        }
    }
}
