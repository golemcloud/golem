// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source Available License v1.1 (the "License");
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

use crate::api::ApiResult;
use crate::services::auth::AuthService;
use crate::services::token::TokenService;
use golem_common::model::account::AccountId;
use golem_common::model::auth::{TokenCreation, TokenWithSecret};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct AdminApi {
    token_service: Arc<TokenService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/admin",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Account
)]
impl AdminApi {
    pub fn new(token_service: Arc<TokenService>, auth_service: Arc<AuthService>) -> Self {
        Self {
            token_service,
            auth_service,
        }
    }

    /// Create an impersonation token for a target account
    ///
    /// Creates a short-lived token that, when used for authentication, produces an
    /// `AdminImpersonation` auth context: access and visibility checks run as the
    /// target account, but audit writes (created_by fields) record the admin's account ID.
    ///
    /// Only users with the `Admin` account role may call this endpoint.
    #[oai(
        path = "/impersonate/:account_id",
        method = "post",
        operation_id = "create_impersonation_token"
    )]
    async fn create_impersonation_token(
        &self,
        account_id: Path<AccountId>,
        request: Json<TokenCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let record = recorded_http_api_request!(
            "create_impersonation_token",
            account_id = account_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_impersonation_token_internal(account_id.0, request.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_impersonation_token_internal(
        &self,
        account_id: AccountId,
        request: TokenCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let result = self
            .token_service
            .create_impersonation_token(account_id, request.expires_at, &auth)
            .await?;
        Ok(Json(result))
    }
}
