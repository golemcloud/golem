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
use super::model::{WebFlowCallbackResponse, WebFlowPollResponse};
use golem_common::api::WebFlowAuthorizeUrlResponse;
use golem_common::model::login::{OAuth2Data, Token, TokenWithSecret};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct LoginApi {}

#[OpenApi(
    prefix_path = "/v1/login",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Login
)]
impl LoginApi {
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
        provider: Query<String>,
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
        _provider: String,
        _access_token: String,
    ) -> ApiResult<Json<TokenWithSecret>> {
        todo!()
    }

    /// Start GitHub OAuth2 interactive flow
    ///
    /// Starts an interactive authorization flow.
    /// The user must open the returned url and enter the userCode in a form before the expires deadline.
    /// Then the finish GitHub OAuth2 interactive flow endpoint must be called with the encoded session to finish the flow.
    #[oai(
        path = "/oauth2/device/start",
        method = "post",
        operation_id = "start_login_oauth2"
    )]
    async fn start_login_oauth2(&self) -> ApiResult<Json<OAuth2Data>> {
        let record = recorded_http_api_request!("start_login_oauth2",);

        let response = self
            .start_login_oauth2_internal()
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn start_login_oauth2_internal(&self) -> ApiResult<Json<OAuth2Data>> {
        todo!()
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
    async fn complete_login_oauth2(
        &self,
        result: Json<String>,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let record = recorded_http_api_request!("complete_login_oauth2",);
        let response = self
            .complete_login_oauth2_internal(result.0)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn complete_login_oauth2_internal(
        &self,
        _result: String,
    ) -> ApiResult<Json<TokenWithSecret>> {
        todo!()
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
        Query(provider): Query<String>,
        /// The redirect URL to redirect to after the user has authorized the application
        Query(redirect): Query<Option<String>>,
    ) -> ApiResult<Json<WebFlowAuthorizeUrlResponse>> {
        let record = recorded_http_api_request!("oauth2_web_flow_start",);
        let response = self
            .oauth2_web_flow_start_internal(provider, redirect)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_web_flow_start_internal(
        &self,
        _provider: String,
        _redirect: Option<String>,
    ) -> ApiResult<Json<WebFlowAuthorizeUrlResponse>> {
        todo!()
    }

    /// GitHub OAuth2 Web Flow callback
    ///
    /// This endpoint handles the callback from GitHub after the user has authorized the application.
    /// It exchanges the code for an access token and then uses that to log the user in.
    #[oai(
        path = "/oauth2/web/callback/github",
        method = "get",
        operation_id = "oauth2_web_flow_callback_github"
    )]
    async fn oauth2_web_flow_callback_github(
        &self,
        /// The authorization code returned by GitHub
        Query(code): Query<String>,
        /// The state parameter for CSRF protection
        Query(state): Query<String>,
    ) -> ApiResult<WebFlowCallbackResponse> {
        let record = recorded_http_api_request!("oauth2_web_flow_callback_github",);
        let response = self
            .oauth2_web_flow_callback_github_internal(code, state)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn oauth2_web_flow_callback_github_internal(
        &self,
        _code: String,
        _state: String,
    ) -> ApiResult<WebFlowCallbackResponse> {
        todo!()
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
        Query(state): Query<String>,
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
        _state: String,
    ) -> ApiResult<WebFlowPollResponse> {
        todo!()
    }
}
