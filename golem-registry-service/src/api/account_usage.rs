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
use crate::api::error::ApiError;
use crate::services::account_resource_override::AccountResourceOverrideService;
use crate::services::account_usage::AccountUsageService;
use crate::services::auth::AuthService;
use golem_common::base_model::api;
use golem_common::model::account::AccountId;
use golem_common::model::account_usage::{
    DEFAULT_STORAGE_USAGE_HISTORY_PERIODS, MemoryLimit, SetMemoryLimit, SetStorageLimit,
    StorageLimit, StorageUsage, StorageUsageHistory,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use golem_service_base::repo::SqlDateTime;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct AccountUsageApi {
    account_usage_service: Arc<AccountUsageService>,
    account_resource_override_service: Arc<AccountResourceOverrideService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/accounts",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Account
)]
impl AccountUsageApi {
    pub fn new(
        account_usage_service: Arc<AccountUsageService>,
        account_resource_override_service: Arc<AccountResourceOverrideService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            account_usage_service,
            account_resource_override_service,
            auth_service,
        }
    }

    /// Get current durable and ephemeral storage usage and storage limit.
    #[oai(
        path = "/:account_id/usage",
        method = "get",
        operation_id = "get_account_storage_usage"
    )]
    async fn get_usage(
        &self,
        account_id: Path<AccountId>,
        period: Query<Option<String>>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<StorageUsage>> {
        let record = recorded_http_api_request!(
            "get_account_storage_usage",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = match period.0 {
            Some(period) => {
                let period = period.parse().map_err(|message: String| {
                    ApiError::bad_request(api::error_code::INVALID_USAGE_PERIOD, message)
                })?;
                self.account_usage_service
                    .get_storage_usage_for_period(account_id.0, period, &auth)
                    .instrument(record.span.clone())
                    .await?
            }
            None => {
                self.account_usage_service
                    .get_storage_usage(account_id.0, &auth)
                    .instrument(record.span.clone())
                    .await?
            }
        };

        record.result(Ok(Json(response)))
    }

    /// Get storage usage for closed billing periods, newest first.
    #[oai(
        path = "/:account_id/usage/history",
        method = "get",
        operation_id = "get_account_storage_usage_history"
    )]
    async fn get_usage_history(
        &self,
        account_id: Path<AccountId>,
        last: Query<Option<usize>>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Vec<StorageUsageHistory>>> {
        let record = recorded_http_api_request!(
            "get_account_storage_usage_history",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_usage_service
            .get_storage_usage_history(
                account_id.0,
                last.0.unwrap_or(DEFAULT_STORAGE_USAGE_HISTORY_PERIODS),
                &auth,
            )
            .instrument(record.span.clone())
            .await?;
        record.result(Ok(Json(response)))
    }

    /// Get effective storage-per-agent override metadata for an account.
    #[oai(
        path = "/:account_id/resource-overrides/max-storage-per-agent",
        method = "get",
        operation_id = "get_account_storage_override"
    )]
    async fn get_storage_override(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<StorageLimit>> {
        let record = recorded_http_api_request!(
            "get_account_storage_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let storage_limit = self
            .account_resource_override_service
            .get_max_disk_space_per_worker(account_id.0, &auth)
            .instrument(record.span.clone())
            .await?;
        record.result(Ok(Json(storage_limit)))
    }

    /// Set a storage-per-agent override for an account. Setting an expiry requires an admin token.
    #[oai(
        path = "/:account_id/resource-overrides/max-storage-per-agent",
        method = "put",
        operation_id = "set_account_storage_override"
    )]
    async fn set_storage_override(
        &self,
        account_id: Path<AccountId>,
        request: Json<SetStorageLimit>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<StorageLimit>> {
        let record = recorded_http_api_request!(
            "set_account_storage_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .set_max_disk_space_per_worker(
                account_id.0,
                request.0.value,
                request.0.expires_at.map(SqlDateTime::new),
                &auth,
            )
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(ApiError::from);

        record.result(response)
    }

    /// Clear a storage-per-agent override for an account.
    #[oai(
        path = "/:account_id/resource-overrides/max-storage-per-agent",
        method = "delete",
        operation_id = "clear_account_storage_override"
    )]
    async fn clear_storage_override(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<StorageLimit>> {
        let record = recorded_http_api_request!(
            "clear_account_storage_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .clear_max_disk_space_per_worker(account_id.0, &auth)
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(ApiError::from);

        record.result(response)
    }

    /// Get the effective maximum linear memory per agent.
    #[oai(
        path = "/:account_id/resource-overrides/max-memory-per-agent",
        method = "get",
        operation_id = "get_account_max_memory_override"
    )]
    async fn get_max_memory_override(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<MemoryLimit>> {
        let record = recorded_http_api_request!(
            "get_account_max_memory_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .get_max_memory_per_worker(account_id.0, &auth)
            .instrument(record.span.clone())
            .await?;
        record.result(Ok(Json(response)))
    }

    /// Set the maximum linear memory per agent. Setting an expiry requires an admin token.
    #[oai(
        path = "/:account_id/resource-overrides/max-memory-per-agent",
        method = "put",
        operation_id = "set_account_max_memory_override"
    )]
    async fn set_max_memory_override(
        &self,
        account_id: Path<AccountId>,
        request: Json<SetMemoryLimit>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<MemoryLimit>> {
        let record = recorded_http_api_request!(
            "set_account_max_memory_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .set_max_memory_per_worker(
                account_id.0,
                request.0.value,
                request.0.expires_at.map(SqlDateTime::new),
                &auth,
            )
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(ApiError::from);
        record.result(response)
    }

    /// Clear the maximum linear memory per-agent override.
    #[oai(
        path = "/:account_id/resource-overrides/max-memory-per-agent",
        method = "delete",
        operation_id = "clear_account_max_memory_override"
    )]
    async fn clear_max_memory_override(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<MemoryLimit>> {
        let record = recorded_http_api_request!(
            "clear_account_max_memory_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .clear_max_memory_per_worker(account_id.0, &auth)
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(ApiError::from);
        record.result(response)
    }

    /// Get the effective monthly memory GB-seconds allowance.
    #[oai(
        path = "/:account_id/resource-overrides/monthly-memory-gb-seconds",
        method = "get",
        operation_id = "get_account_monthly_memory_override"
    )]
    async fn get_monthly_memory_override(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<MemoryLimit>> {
        let record = recorded_http_api_request!(
            "get_account_monthly_memory_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .get_monthly_memory_gb_seconds(account_id.0, &auth)
            .instrument(record.span.clone())
            .await?;
        record.result(Ok(Json(response)))
    }

    /// Set the monthly memory GB-seconds allowance. Setting an expiry requires an admin token.
    #[oai(
        path = "/:account_id/resource-overrides/monthly-memory-gb-seconds",
        method = "put",
        operation_id = "set_account_monthly_memory_override"
    )]
    async fn set_monthly_memory_override(
        &self,
        account_id: Path<AccountId>,
        request: Json<SetMemoryLimit>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<MemoryLimit>> {
        let record = recorded_http_api_request!(
            "set_account_monthly_memory_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .set_monthly_memory_gb_seconds(
                account_id.0,
                request.0.value,
                request.0.expires_at.map(SqlDateTime::new),
                &auth,
            )
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(ApiError::from);
        record.result(response)
    }

    /// Clear the monthly memory GB-seconds allowance override.
    #[oai(
        path = "/:account_id/resource-overrides/monthly-memory-gb-seconds",
        method = "delete",
        operation_id = "clear_account_monthly_memory_override"
    )]
    async fn clear_monthly_memory_override(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<MemoryLimit>> {
        let record = recorded_http_api_request!(
            "clear_account_monthly_memory_override",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let response = self
            .account_resource_override_service
            .clear_monthly_memory_gb_seconds(account_id.0, &auth)
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(ApiError::from);
        record.result(response)
    }
}
