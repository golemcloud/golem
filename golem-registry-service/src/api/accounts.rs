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
use golem_common_next::api::Page;
use golem_common_next::model::account::{Account, AccountData, Plan};
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::model::{AccountId, Empty};
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use param::Query;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct AccountsApi {}

#[OpenApi(prefix_path = "/v1/accounts", tag = ApiTags::Account)]
impl AccountsApi {
    /// Find accounts
    ///
    /// Find matching accounts. Only your own account or accounts you have at least one grant from will be returned
    #[oai(path = "/", method = "get", operation_id = "find_accounts")]
    async fn find_accounts(
        &self,
        email: Query<Option<String>>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Account>>> {
        let record = recorded_http_api_request!("find_accounts", email = email.0);

        let auth = AuthCtx::new(token.secret());

        let response = self
            .find_accounts_internal(email.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn find_accounts_internal(
        &self,
        _email: Option<String>,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Account>>> {
        todo!()
    }

    /// Create account
    ///
    /// Create a new account. The response is the created account data.
    #[oai(path = "/", method = "post", operation_id = "create_account")]
    async fn post_account(
        &self,
        data: Json<AccountData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record = recorded_http_api_request!("create_account", account_name = data.name.clone());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .post_account_internal(data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn post_account_internal(
        &self,
        _data: AccountData,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        todo!()
    }

    /// Get account
    ///
    /// Retrieve an account for a given Account ID
    #[oai(path = "/:account_id", method = "get", operation_id = "get_account")]
    async fn get_account(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("get_account", account_id = account_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_account_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_internal(
        &self,
        _account_id: AccountId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        todo!()
    }

    /// Get account's plan
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

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_account_plan_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_account_plan_internal(
        &self,
        _account_id: AccountId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Plan>> {
        todo!()
    }

    /// Update account
    ///
    /// Allows the user to change the account details such as name and email.
    ///
    /// Changing the planId is not allowed and the request will be rejected.
    /// The response is the updated account data.
    #[oai(path = "/:account_id", method = "put", operation_id = "update_account")]
    async fn put_account(
        &self,
        account_id: Path<AccountId>,
        data: Json<AccountData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Account>> {
        let record =
            recorded_http_api_request!("update_account", account_id = account_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .put_account_internal(account_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn put_account_internal(
        &self,
        _account_id: AccountId,
        _data: AccountData,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Account>> {
        todo!()
    }

    /// Delete account
    ///
    /// Delete an account.
    #[oai(
        path = "/:account_id",
        method = "delete",
        operation_id = "delete_account"
    )]
    async fn delete_account(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record =
            recorded_http_api_request!("delete_account", account_id = account_id.0.to_string());

        let auth = AuthCtx::new(token.secret());

        let response = self
            .delete_account_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_account_internal(
        &self,
        _account_id: AccountId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Empty>> {
        todo!()
    }
}
