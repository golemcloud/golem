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
use crate::services::auth::AuthService;
use crate::services::token::TokenService;
use golem_common::model::Empty;
use golem_common::model::auth::{Token, TokenId};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct TokensApi {
    token_service: Arc<TokenService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/tokens",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Token
)]
impl TokensApi {
    pub fn new(token_service: Arc<TokenService>, auth_service: Arc<AuthService>) -> Self {
        Self {
            token_service,
            auth_service,
        }
    }

    /// Get token by id
    #[oai(path = "/:token_id", method = "get", operation_id = "get_token")]
    async fn get_token(
        &self,
        token_id: Path<TokenId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Token>> {
        let record = recorded_http_api_request!("get_token", token_id = token_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_token_internal(token_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_token_internal(&self, token_id: TokenId, auth: AuthCtx) -> ApiResult<Json<Token>> {
        let result = self.token_service.get(token_id, &auth).await?;

        Ok(Json(result.without_secret()))
    }

    #[oai(path = "/:token_id", method = "delete", operation_id = "delete_token")]
    /// Delete a token
    ///
    /// Deletes a previously created token given by its identifier.
    async fn delete_token(
        &self,
        token_id: Path<TokenId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record = recorded_http_api_request!("delete_token", token_id = token_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_token_internal(token_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_token_internal(
        &self,
        token_id: TokenId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Empty>> {
        self.token_service.delete(token_id, &auth).await?;
        Ok(Json(Empty {}))
    }
}
