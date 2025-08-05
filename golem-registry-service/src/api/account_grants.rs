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
use golem_common::model::auth::Role;
use golem_common::model::{AccountId, Empty};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use tracing::Instrument;

pub struct AccountGrantsApi {}

#[OpenApi(prefix_path = "/v1/accounts", tag = ApiTags::Account, tag = ApiTags::Grant)]
impl AccountGrantsApi {
    #[oai(
        path = "/:account_id/grants",
        method = "get",
        operation_id = "get_account_grants"
    )]
    async fn get_grants(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Vec<Role>>> {
        let record =
            recorded_http_api_request!("get_account_grants", account_id = account_id.0.to_string());
        let response = self
            .get_grants_internal(account_id.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_grants_internal(
        &self,
        _account_id: AccountId,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Vec<Role>>> {
        todo!()
    }

    #[oai(
        path = "/:account_id/grants/:role",
        method = "get",
        operation_id = "get_account_grant"
    )]
    async fn get_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        let record =
            recorded_http_api_request!("get_account_grant", account_id = account_id.0.to_string());
        let response = self
            .get_grant_internal(account_id.0, role.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_grant_internal(
        &self,
        _account_id: AccountId,
        _role: Role,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        todo!()
    }

    #[oai(
        path = "/:account_id/grants/:role",
        method = "put",
        operation_id = "create_account_grant"
    )]
    async fn put_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        let record = recorded_http_api_request!(
            "create_account_grant",
            account_id = account_id.0.to_string()
        );
        let response = self
            .put_grant_internal(account_id.0, role.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn put_grant_internal(
        &self,
        _account_id: AccountId,
        _role: Role,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        todo!()
    }

    #[oai(
        path = "/:account_id/grants/:role",
        method = "delete",
        operation_id = "delete_account_grant"
    )]
    async fn delete_grant(
        &self,
        account_id: Path<AccountId>,
        role: Path<Role>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        let record = recorded_http_api_request!(
            "delete_account_grant",
            account_id = account_id.0.to_string()
        );
        let response = self
            .delete_grant_internal(account_id.0, role.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_grant_internal(
        &self,
        _account_id: AccountId,
        _role: Role,
        _token: GolemSecurityScheme,
    ) -> ApiResult<Json<Empty>> {
        todo!()
    }
}
