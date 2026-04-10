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
use crate::services::auth::AuthService;
use crate::services::retry_policy::RetryPolicyService;
use golem_common::model::Page;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::retry_policy::{
    RetryPolicyCreation, RetryPolicyDto, RetryPolicyId, RetryPolicyRevision, RetryPolicyUpdate,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct RetryPoliciesApi {
    retry_policy_service: Arc<RetryPolicyService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::RetryPolicies
)]
impl RetryPoliciesApi {
    pub fn new(
        retry_policy_service: Arc<RetryPolicyService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            retry_policy_service,
            auth_service,
        }
    }

    /// Create a new retry policy
    #[oai(
        path = "/envs/:environment_id/retry-policies",
        method = "post",
        operation_id = "create_retry_policy",
        tag = ApiTags::Environment
    )]
    async fn create_retry_policy(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<RetryPolicyCreation>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let record = recorded_http_api_request!(
            "create_retry_policy",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .create_retry_policy_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_retry_policy_internal(
        &self,
        environment_id: EnvironmentId,
        payload: RetryPolicyCreation,
        auth: AuthCtx,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let result = self
            .retry_policy_service
            .create(environment_id, payload, &auth)
            .await?;

        Ok(Json(result.into()))
    }

    /// Get all retry policies of the environment
    #[oai(
        path = "/envs/:environment_id/retry-policies",
        method = "get",
        operation_id = "get_environment_retry_policies",
        tag = ApiTags::Environment
    )]
    async fn get_environment_retry_policies(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<RetryPolicyDto>>> {
        let record = recorded_http_api_request!(
            "get_environment_retry_policies",
            environment_id = environment_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_retry_policies_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_retry_policies_internal(
        &self,
        environment_id: EnvironmentId,
        auth: AuthCtx,
    ) -> ApiResult<Json<Page<RetryPolicyDto>>> {
        let result = self
            .retry_policy_service
            .list_in_environment(environment_id, &auth)
            .await?;

        let converted = result.into_iter().map(RetryPolicyDto::from).collect();

        Ok(Json(Page { values: converted }))
    }

    /// Get retry policy by id.
    #[oai(
        path = "/retry-policies/:retry_policy_id",
        method = "get",
        operation_id = "get_retry_policy"
    )]
    pub async fn get_retry_policy(
        &self,
        retry_policy_id: Path<RetryPolicyId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let record = recorded_http_api_request!(
            "get_retry_policy",
            retry_policy_id = retry_policy_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_retry_policy_internal(retry_policy_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_retry_policy_internal(
        &self,
        retry_policy_id: RetryPolicyId,
        auth: AuthCtx,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let result = self
            .retry_policy_service
            .get(retry_policy_id, &auth)
            .await?;
        Ok(Json(result.into()))
    }

    /// Update retry policy
    #[oai(
        path = "/retry-policies/:retry_policy_id",
        method = "patch",
        operation_id = "update_retry_policy"
    )]
    pub async fn update_retry_policy(
        &self,
        retry_policy_id: Path<RetryPolicyId>,
        data: Json<RetryPolicyUpdate>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let record = recorded_http_api_request!(
            "update_retry_policy",
            retry_policy_id = retry_policy_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_retry_policy_internal(retry_policy_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_retry_policy_internal(
        &self,
        retry_policy_id: RetryPolicyId,
        data: RetryPolicyUpdate,
        auth: AuthCtx,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let result = self
            .retry_policy_service
            .update(retry_policy_id, data, &auth)
            .await?;
        Ok(Json(result.into()))
    }

    /// Delete retry policy
    #[oai(
        path = "/retry-policies/:retry_policy_id",
        method = "delete",
        operation_id = "delete_retry_policy"
    )]
    pub async fn delete_retry_policy(
        &self,
        retry_policy_id: Path<RetryPolicyId>,
        current_revision: Query<RetryPolicyRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let record = recorded_http_api_request!(
            "delete_retry_policy",
            retry_policy_id = retry_policy_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_retry_policy_internal(retry_policy_id.0, current_revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_retry_policy_internal(
        &self,
        retry_policy_id: RetryPolicyId,
        current_revision: RetryPolicyRevision,
        auth: AuthCtx,
    ) -> ApiResult<Json<RetryPolicyDto>> {
        let result = self
            .retry_policy_service
            .delete(retry_policy_id, current_revision, &auth)
            .await?;
        Ok(Json(result.into()))
    }
}
