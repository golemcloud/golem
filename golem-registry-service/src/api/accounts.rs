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
use crate::services::account::AccountService;
use crate::services::auth::AuthService;
use crate::services::plan::PlanService;
use crate::services::token::TokenService;
use golem_common::model::Empty;
use golem_common::model::Page;
use golem_common::model::account::{
    Account, AccountCreation, AccountId, AccountRevision, AccountSetPlan, AccountSetRoles,
    AccountUpdate,
};
use golem_common::model::auth::{Token, TokenCreation, TokenWithSecret};
use golem_common::model::plan::Plan;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct AccountsApi {
    account_service: Arc<AccountService>,
    plan_service: Arc<PlanService>,
    token_service: Arc<TokenService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/accounts",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Account
)]
impl AccountsApi {
    pub fn new(
        account_service: Arc<AccountService>,
        plan_service: Arc<PlanService>,
        token_service: Arc<TokenService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            account_service,
            plan_service,
            token_service,
            auth_service,
        }
    }

    /// Create a new account. The response is the created account data.
    #[oai(path = "/", method = "post", operation_id = "create_account")]
    async fn create_account(
        &self,
        data: Json<AccountCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record = recorded_http_api_request!("create_account", account_name = data.name.clone());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_account_internal(data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_account_internal(
        &self,
        data: AccountCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self.account_service.create(data, &auth).await?;
        Ok(Json(result))
    }

    /// Retrieve an account for a given Account ID
    #[oai(path = "/:account_id", method = "get", operation_id = "get_account")]
    async fn get_account(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("get_account", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self.account_service.get(account_id, &auth).await?;
        Ok(Json(result))
    }

    /// Get an account's plan
    #[oai(
        path = "/:account_id/plan",
        method = "get",
        operation_id = "get_account_plan"
    )]
    async fn get_account_plan(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Plan>> {
        let record =
            recorded_http_api_request!("get_account_plan", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_account_plan_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_plan_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Plan>> {
        let account = self.account_service.get(account_id, &auth).await?;
        let plan = self.plan_service.get(&account.plan_id, &auth).await?;

        Ok(Json(plan))
    }

    /// Update account
    ///
    /// Allows the user to change the account details such as name and email.
    ///
    /// Changing the planId is not allowed and the request will be rejected.
    /// The response is the updated account data.
    #[oai(
        path = "/:account_id",
        method = "patch",
        operation_id = "update_account"
    )]
    async fn update_account(
        &self,
        account_id: Path<AccountId>,
        data: Json<AccountUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("update_account", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_account_internal(account_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_account_internal(
        &self,
        account_id: AccountId,
        data: AccountUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self.account_service.update(account_id, data, &auth).await?;
        Ok(Json(result))
    }

    /// Set the roles of an account
    #[oai(
        path = "/:account_id/roles",
        method = "put",
        operation_id = "set_account_roles"
    )]
    async fn set_account_roles(
        &self,
        account_id: Path<AccountId>,
        payload: Json<AccountSetRoles>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("set_account_roles", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .set_account_roles_internal(account_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn set_account_roles_internal(
        &self,
        account_id: AccountId,
        payload: AccountSetRoles,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self
            .account_service
            .set_roles(account_id, payload, &auth)
            .await?;
        Ok(Json(result))
    }

    /// Set the plan of an account
    #[oai(
        path = "/:account_id/plan",
        method = "put",
        operation_id = "set_account_plan"
    )]
    async fn set_account_plan(
        &self,
        account_id: Path<AccountId>,
        payload: Json<AccountSetPlan>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("set_account_plan", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .set_account_plan_internal(account_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn set_account_plan_internal(
        &self,
        account_id: AccountId,
        payload: AccountSetPlan,
        auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        let result = self
            .account_service
            .set_plan(account_id, payload, &auth)
            .await?;
        Ok(Json(result))
    }

    /// List all tokens of an account.
    /// The format of each element is the same as the data object in the oauth2 endpoint's response.
    #[oai(
        path = "/:account_id/tokens",
        method = "get",
        operation_id = "list_account_tokens",
        tag = ApiTags::Token
    )]
    async fn list_account_tokens(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Token>>> {
        let record = recorded_http_api_request!(
            "list_account_tokens",
            account_id = account_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_account_tokens_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_account_tokens_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<Token>>> {
        let tokens = self
            .token_service
            .list_in_account(account_id, &auth)
            .await?;
        Ok(Json(Page {
            values: tokens.into_iter().map(|t| t.without_secret()).collect(),
        }))
    }

    #[oai(
        path = "/:account_id/tokens",
        method = "post",
        operation_id = "create_token",
        tag = ApiTags::Token
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
        request: Json<TokenCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let record =
            recorded_http_api_request!("create_token", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_token_internal(account_id.0, request.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_token_internal(
        &self,
        account_id: AccountId,
        request: TokenCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<TokenWithSecret>> {
        let result = self
            .token_service
            .create(account_id, request.expires_at, &auth)
            .await?;

        Ok(Json(result))
    }

    /// Delete an account.
    #[oai(
        path = "/:account_id",
        method = "delete",
        operation_id = "delete_account"
    )]
    async fn delete_account(
        &self,
        account_id: Path<AccountId>,
        current_revision: Query<AccountRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record =
            recorded_http_api_request!("delete_account", account_id = account_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_account_internal(account_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_account_internal(
        &self,
        account_id: AccountId,
        current_revision: AccountRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<Empty>> {
        self.account_service
            .delete(account_id, current_revision, &auth)
            .await?;
        Ok(Json(Empty {}))
    }
}
