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

use crate::api::{ApiError, ApiResult, ApiTags};
use crate::model::*;
use crate::service::account_grant::{AccountGrantService, AccountGrantServiceError};
use crate::service::auth::AuthService;
use golem_common::model::auth::{AccountAction, Role};
use golem_common::model::error::ErrorBody;
use golem_common::model::AccountId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct GrantApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub account_grant_service: Arc<dyn AccountGrantService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/accounts", tag = ApiTags::Grant)]
impl GrantApi {
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
        account_id: AccountId,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Vec<Role>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        self.auth_service
            .authorize_account_action(&auth, &account_id, &AccountAction::ViewAccountGrants)
            .await?;

        let response = self.account_grant_service.get(&account_id).await?;
        Ok(Json(response))
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
        account_id: AccountId,
        role: Role,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        self.auth_service
            .authorize_account_action(&auth, &account_id, &AccountAction::ViewAccountGrants)
            .await?;

        let roles = self.account_grant_service.get(&account_id).await?;

        if roles.contains(&role) {
            Ok(Json(role))
        } else {
            Err(ApiError::NotFound(Json(ErrorBody {
                error: "Role not found".to_string(),
            })))
        }
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
        account_id: AccountId,
        role: Role,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Role>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        self.auth_service
            .authorize_account_action(&auth, &account_id, &AccountAction::CreateAccountGrant)
            .await?;

        self.account_grant_service.add(&account_id, &role).await?;

        Ok(Json(role))
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
    ) -> ApiResult<Json<DeleteGrantResponse>> {
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
        account_id: AccountId,
        role: Role,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<DeleteGrantResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        self.auth_service
            .authorize_account_action(&auth, &account_id, &AccountAction::DeleteAccountGrant)
            .await?;

        if auth.token.account_id == account_id && role == Role::Admin {
            Err(AccountGrantServiceError::ArgValidation(vec![
                "Cannot remove Admin role from current account.".to_string(),
            ]))?
        };

        self.account_grant_service
            .remove(&account_id, &role)
            .await?;

        Ok(Json(DeleteGrantResponse {}))
    }
}
