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
use crate::services::auth::AuthService;
use crate::services::environment::EnvironmentService;
use crate::services::token::{TokenError, TokenService};
use golem_common::model::Page;
use golem_common::model::account::AccountEmail;
use golem_common::model::application::ApplicationName;
use golem_common::model::auth::Token;
use golem_common::model::environment::{EnvironmentName, EnvironmentWithDetails};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct MeApi {
    token_service: Arc<TokenService>,
    environment_service: Arc<EnvironmentService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/me",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Me
)]
impl MeApi {
    pub fn new(
        token_service: Arc<TokenService>,
        environment_service: Arc<EnvironmentService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            token_service,
            environment_service,
            auth_service,
        }
    }

    /// Gets information about the current token.
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

    async fn current_token_internal(&self, token: GolemSecurityScheme) -> ApiResult<Json<Token>> {
        // This route is a bit special, get the token directly instead of doing the normal auth flow.
        let token_info = self
            .token_service
            .get_by_secret(&token.secret(), &AuthCtx::system())
            .await
            .map_err(|err| match err {
                TokenError::TokenBySecretNotFound => ApiError::unauthorized(
                    golem_common::base_model::api::error_code::AUTH_UNAUTHORIZED,
                    "Token not found".to_string(),
                ),
                other => other.into(),
            })?;

        Ok(Json(token_info.without_secret()))
    }

    /// List all environments that are visible to the current user, either directly or through shares.
    #[oai(
        path = "/visible-environments",
        method = "get",
        operation_id = "list_visible_environments"
    )]
    pub async fn list_visible_environments(
        &self,
        token: GolemSecurityScheme,
        account_email: Query<Option<AccountEmail>>,
        app_name: Query<Option<ApplicationName>>,
        env_name: Query<Option<EnvironmentName>>,
    ) -> ApiResult<Json<Page<EnvironmentWithDetails>>> {
        let record = recorded_http_api_request!("list_visible_environments",);

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_visible_environments_internal(account_email.0, app_name.0, env_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_visible_environments_internal(
        &self,
        account_email: Option<AccountEmail>,
        app_name: Option<ApplicationName>,
        env_name: Option<EnvironmentName>,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<EnvironmentWithDetails>>> {
        let environments = self
            .environment_service
            .list_visible_environments(
                account_email.as_ref(),
                app_name.as_ref(),
                env_name.as_ref(),
                &auth,
            )
            .await?;
        Ok(Json(Page {
            values: environments,
        }))
    }
}
