// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use crate::bootstrap::login::{LoginSystem, LoginSystemEnabled};
use crate::model::login::WebflowKind;
use crate::services::oauth2::WebflowCallbackAction;
use golem_common::base_model::api;
use golem_common::model::Empty;
use golem_common::model::auth::TokenWithSecret;
use golem_common::model::login::{
    OAuth2Provider, OAuth2WebflowData, OAuth2WebflowStart, OAuth2WebflowStateId,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct LoginApi {
    login_system: LoginSystem,
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

    /// Acquire token with OAuth2 authorization
    ///
    /// Gets a token by authorizing with an external OAuth2 provider. Currently only github is supported.
    ///
    /// In the response:
    /// - `id` is the identifier of the token itself
    /// - `accountId` is the account's identifier, can be used on the account API
    /// - `secret` is the secret key to be sent in the Authorization header as a bearer token for all the other endpoints
    ///
    #[oai(path = "/oauth2", method = "post", operation_id = "login_oauth2")]
    async fn oauth2(
        &self,
        /// Currently only `github` is supported.
        provider: Query<OAuth2Provider>,
        /// OAuth2 access token
        #[oai(name = "access-token")]
        access_token: Query<String>,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let record = recorded_http_api_request!("login_oauth2",);
        let response = self
            .oauth2_internal(provider.0, access_token.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_internal(
        &self,
        provider: OAuth2Provider,
        access_token: String,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let login_system = self.get_enabled_login_system()?;

        let result = login_system
            .oauth2_service
            .exchange_external_access_token_for_token(&provider, &access_token)
            .await?;

        Ok(Json(result))
    }

    /// Initiate OAuth2 Web Flow
    ///
    /// Starts the OAuth2 web flow. Two flow kinds are supported:
    ///
    /// - `browser`: The callback will immediately redirect to the given URL with the
    ///   Golem token secret appended as a `token` query parameter. Intended for
    ///   browser-based frontends.
    ///
    /// - `cli`: The callback stores the token in the session. The client polls the
    ///   poll endpoint with the returned state id to retrieve the token once available.
    ///   Intended for CLI tools and headless environments.
    #[oai(
        path = "/oauth2/web/authorize",
        method = "post",
        operation_id = "start_oauth2_webflow"
    )]
    async fn start_oauth2_webflow(
        &self,
        request: Json<OAuth2WebflowStart>,
    ) -> ApiResult<Json<OAuth2WebflowData>> {
        let record = recorded_http_api_request!("start_oauth2_webflow",);
        let response = self
            .start_oauth2_webflow_internal(request.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn start_oauth2_webflow_internal(
        &self,
        request: OAuth2WebflowStart,
    ) -> ApiResult<Json<OAuth2WebflowData>> {
        let login_system = self.get_enabled_login_system()?;

        let (provider, kind) = match request {
            OAuth2WebflowStart::Browser(r) => (
                r.provider,
                WebflowKind::Browser {
                    redirect: r.redirect,
                },
            ),
            OAuth2WebflowStart::Cli(r) => (r.provider, WebflowKind::Cli),
        };

        let result = login_system
            .oauth2_service
            .start_webflow(&provider, kind)
            .await?;

        Ok(Json(result))
    }

    /// OAuth2 Web Flow callback
    ///
    /// This endpoint handles the callback from the provider after the user has authorized the application.
    /// It exchanges the code for an access token and then uses that to log the user in.
    #[oai(
        path = "/oauth2/web/callback",
        method = "get",
        operation_id = "submit_oauth2_webflow_callback"
    )]
    async fn submit_oauth2_webflow_callback(
        &self,
        /// The authorization code returned by GitHub
        Query(code): Query<String>,
        /// The state parameter for CSRF protection
        Query(state): Query<OAuth2WebflowStateId>,
    ) -> ApiResult<WebFlowCallbackResponse> {
        let record = recorded_http_api_request!("submit_oauth2_webflow_callback",);
        let response = self
            .submit_oauth2_webflow_callback_internal(code, state)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn submit_oauth2_webflow_callback_internal(
        &self,
        code: String,
        state: OAuth2WebflowStateId,
    ) -> ApiResult<WebFlowCallbackResponse> {
        let login_system = self.get_enabled_login_system()?;

        let action = login_system
            .oauth2_service
            .handle_webflow_callback(&state, code)
            .await?;

        let redirect = match action {
            WebflowCallbackAction::BrowserRedirect { redirect } => redirect,
            WebflowCallbackAction::CliRedirect { redirect } => redirect,
        };

        Ok(WebFlowCallbackResponse::Redirect(
            Json(Empty {}),
            redirect.to_string(),
        ))
    }

    /// Poll for OAuth2 Web Flow token
    ///
    /// This endpoint is used by clients to poll for the token after the user has authorized the application via the web flow.
    /// A given state might only be exchanged for a token once. Any further attempts to exchange the state will fail.
    #[oai(
        path = "/oauth2/web/poll",
        method = "get",
        operation_id = "poll_oauth2_webflow"
    )]
    async fn poll_oauth2_webflow(
        &self,
        /// The state parameter for identifying the session
        Query(state): Query<OAuth2WebflowStateId>,
    ) -> ApiResult<WebFlowPollResponse> {
        let record = recorded_http_api_request!("poll_oauth2_webflow",);
        let response = self
            .poll_oauth2_webflow_internal(state)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn poll_oauth2_webflow_internal(
        &self,
        state_id: OAuth2WebflowStateId,
    ) -> ApiResult<WebFlowPollResponse> {
        let login_system = self.get_enabled_login_system()?;

        let state = login_system
            .oauth2_service
            .exchange_webflow_state_for_token(&state_id)
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
            LoginSystem::Disabled => Err(ApiError::forbidden(
                api::error_code::FEATURE_DISABLED,
                "Logins are disabled by configuration".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone, ApiResponse)]
enum WebFlowPollResponse {
    /// OAuth flow has completed
    #[oai(status = 200)]
    Completed(Json<TokenWithSecret>),
    /// OAuth flow is pending
    #[oai(status = 202)]
    Pending(Json<Empty>),
}

#[derive(Debug, Clone, ApiResponse)]
enum WebFlowCallbackResponse {
    /// Redirect to the given URL after completing the OAuth flow
    #[oai(status = 302)]
    Redirect(Json<Empty>, #[oai(header = "Location")] String),
}
