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
use crate::services::auth::AuthService;
use crate::services::permission_share::PermissionShareService;
use golem_common::model::Page;
use golem_common::model::account::AccountId;
use golem_common::model::permission_share::{
    PermissionShare, PermissionShareCreation, PermissionShareId, PermissionShareRevision,
    PermissionShareUpdate,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::{AuthCtx, GolemSecurityScheme};
use poem_openapi::OpenApi;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct PermissionSharesApi {
    permission_share_service: Arc<PermissionShareService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::PermissionShares
)]
impl PermissionSharesApi {
    pub fn new(
        permission_share_service: Arc<PermissionShareService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            permission_share_service,
            auth_service,
        }
    }

    /// Create a new permission share owned by an account.
    #[oai(
        path = "/accounts/:account_id/permission-shares",
        method = "post",
        operation_id = "create_permission_share",
        tag = ApiTags::Account,
    )]
    async fn create_permission_share(
        &self,
        account_id: Path<AccountId>,
        payload: Json<PermissionShareCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PermissionShare>> {
        let record = recorded_http_api_request!(
            "create_permission_share",
            account_id = account_id.0.to_string(),
            target_account_id = payload.0.target_account_id.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_permission_share_internal(account_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_permission_share_internal(
        &self,
        account_id: AccountId,
        payload: PermissionShareCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<PermissionShare>> {
        Ok(Json(
            self.permission_share_service
                .create(account_id, payload, &auth)
                .await?,
        ))
    }

    /// List permission shares owned by an account.
    #[oai(
        path = "/accounts/:account_id/permission-shares",
        method = "get",
        operation_id = "list_owned_permission_shares",
        tag = ApiTags::Account,
    )]
    async fn list_owned_permission_shares(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<PermissionShare>>> {
        let record = recorded_http_api_request!(
            "list_owned_permission_shares",
            account_id = account_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_owned_permission_shares_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_owned_permission_shares_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<PermissionShare>>> {
        Ok(Json(Page {
            values: self
                .permission_share_service
                .get_for_owner(account_id, &auth)
                .await?,
        }))
    }

    /// List permission shares targeting an account.
    #[oai(
        path = "/accounts/:account_id/received-permission-shares",
        method = "get",
        operation_id = "list_received_permission_shares",
        tag = ApiTags::Account,
    )]
    async fn list_received_permission_shares(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<PermissionShare>>> {
        let record = recorded_http_api_request!(
            "list_received_permission_shares",
            account_id = account_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .list_received_permission_shares_internal(account_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn list_received_permission_shares_internal(
        &self,
        account_id: AccountId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<PermissionShare>>> {
        Ok(Json(Page {
            values: self
                .permission_share_service
                .get_for_target(account_id, &auth)
                .await?,
        }))
    }

    /// Get permission share by id.
    #[oai(
        path = "/permission-shares/:permission_share_id",
        method = "get",
        operation_id = "get_permission_share"
    )]
    async fn get_permission_share(
        &self,
        permission_share_id: Path<PermissionShareId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PermissionShare>> {
        let record = recorded_http_api_request!(
            "get_permission_share",
            permission_share_id = permission_share_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_permission_share_internal(permission_share_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_permission_share_internal(
        &self,
        permission_share_id: PermissionShareId,
        auth: AuthCtx,
    ) -> ApiResult<Json<PermissionShare>> {
        Ok(Json(
            self.permission_share_service
                .get(permission_share_id, &auth)
                .await?,
        ))
    }

    /// Get permission share by owner account and name.
    #[oai(
        path = "/accounts/:account_id/permission-shares/:name",
        method = "get",
        operation_id = "get_permission_share_by_name",
        tag = ApiTags::Account,
    )]
    async fn get_permission_share_by_name(
        &self,
        account_id: Path<AccountId>,
        name: Path<String>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PermissionShare>> {
        let record = recorded_http_api_request!(
            "get_permission_share_by_name",
            account_id = account_id.0.to_string(),
            name = name.0.clone()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_permission_share_by_name_internal(account_id.0, name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_permission_share_by_name_internal(
        &self,
        account_id: AccountId,
        name: String,
        auth: AuthCtx,
    ) -> ApiResult<Json<PermissionShare>> {
        Ok(Json(
            self.permission_share_service
                .get_by_owner_and_name(account_id, &name, &auth)
                .await?,
        ))
    }

    /// Update permission share data.
    #[oai(
        path = "/permission-shares/:permission_share_id",
        method = "patch",
        operation_id = "update_permission_share"
    )]
    async fn update_permission_share(
        &self,
        permission_share_id: Path<PermissionShareId>,
        payload: Json<PermissionShareUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PermissionShare>> {
        let record = recorded_http_api_request!(
            "update_permission_share",
            permission_share_id = permission_share_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_permission_share_internal(permission_share_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_permission_share_internal(
        &self,
        permission_share_id: PermissionShareId,
        payload: PermissionShareUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<PermissionShare>> {
        Ok(Json(
            self.permission_share_service
                .update(permission_share_id, payload, &auth)
                .await?,
        ))
    }

    /// Delete permission share.
    #[oai(
        path = "/permission-shares/:permission_share_id",
        method = "delete",
        operation_id = "delete_permission_share"
    )]
    async fn delete_permission_share(
        &self,
        permission_share_id: Path<PermissionShareId>,
        current_revision: Query<PermissionShareRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<PermissionShare>> {
        let record = recorded_http_api_request!(
            "delete_permission_share",
            permission_share_id = permission_share_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_permission_share_internal(permission_share_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_permission_share_internal(
        &self,
        permission_share_id: PermissionShareId,
        current_revision: PermissionShareRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<PermissionShare>> {
        Ok(Json(
            self.permission_share_service
                .delete(permission_share_id, current_revision, &auth)
                .await?,
        ))
    }
}
