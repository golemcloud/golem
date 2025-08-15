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

use crate::api::ApiResult;
use golem_common::api::CreateTokenRequest;
use golem_common::model::account::AccountId;
use golem_common::model::auth::{AuthCtx, Token, TokenWithSecret};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;
use crate::services::token::TokenService;
use std::sync::Arc;

pub struct AccountTokensApi {
    token_service: Arc<TokenService>
}

#[OpenApi(
    prefix_path = "/v1/accounts",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Account,
    tag = ApiTags::Token
)]
impl AccountTokensApi {
    pub fn new(
        token_service: Arc<TokenService>
    ) -> Self {
        Self { token_service }
    }

    /// Get all tokens
    ///
    /// Gets all created tokens of an account.
    /// The format of each element is the same as the data object in the oauth2 endpoint's response.
    #[oai(
        path = "/:account_id/tokens",
        method = "get",
        operation_id = "get_account_tokens"
    )]
    async fn get_tokens(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Vec<Token>>> {
        let record =
            recorded_http_api_request!("get_account_tokens", account_id = account_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_tokens_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_tokens_internal(
        &self,
        _account_id: AccountId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Vec<Token>>> {
        todo!()
    }

    #[oai(
        path = "/:account_id/tokens",
        method = "post",
        operation_id = "create_token"
    )]
    /// Create new token
    ///
    /// Creates a new token with a given expiration date.
    /// The response not only contains the token data but also the secret which can be passed as a bearer token to the Authorization header to the Golem Cloud REST API.
    ///
    // Note that this is the only time this secret is returned!
    async fn create_token(
        &self,
        account_id: Path<AccountId>,
        request: Json<CreateTokenRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let record =
            recorded_http_api_request!("create_token", account_id = account_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_token_internal(account_id.0, request.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_token_internal(
        &self,
        account_id: AccountId,
        request: CreateTokenRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let result = self.token_service.create(account_id, request.expires_at).await?;
        Ok(Json(result))
    }
}
