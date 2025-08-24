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
use crate::model::auth::AuthCtx;
use crate::services::auth::AuthService;
use crate::services::environment_share::EnvironmentShareService;
use golem_common::model::account::AccountId;
use golem_common::model::environment_share::{
    EnvironmentShare, EnvironmentShareId, UpdateEnvironmentShare,
};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;
use uuid::Uuid;

pub struct EnvironmentSharesApi {
    environment_share_service: Arc<EnvironmentShareService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/environment-shares",
    tag = ApiTags::RegistryService,
    tag = ApiTags::EnvironmentShares
)]
impl EnvironmentSharesApi {
    pub fn new(
        environment_share_service: Arc<EnvironmentShareService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            environment_share_service,
            auth_service,
        }
    }

    /// Get environment share by id.
    #[oai(
        path = "/:environment_share_id",
        method = "get",
        operation_id = "get_environment_share"
    )]
    pub async fn get_environment_share(
        &self,
        environment_share_id: Path<EnvironmentShareId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let record = recorded_http_api_request!(
            "get_environment_share",
            environment_share_id = environment_share_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_environment_share_internal(environment_share_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_share_internal(
        &self,
        environment_share_id: EnvironmentShareId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let share = self
            .environment_share_service
            .get(&environment_share_id)
            .await?;
        Ok(Json(share))
    }

    /// Update environment share
    #[oai(
        path = "/:environment_share_id",
        method = "patch",
        operation_id = "update_environment_share"
    )]
    pub async fn update_environment_share(
        &self,
        environment_share_id: Path<EnvironmentShareId>,
        data: Json<UpdateEnvironmentShare>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let record = recorded_http_api_request!(
            "update_environment_share",
            environment_share_id = environment_share_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_environment_share_internal(environment_share_id.0, data.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_environment_share_internal(
        &self,
        environment_share_id: EnvironmentShareId,
        data: UpdateEnvironmentShare,
        _auth: AuthCtx,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let actor = AccountId(Uuid::new_v4());
        let share = self
            .environment_share_service
            .update(&environment_share_id, data, actor)
            .await?;
        Ok(Json(share))
    }

    /// Delete environment share
    #[oai(
        path = "/:environment_share_id",
        method = "delete",
        operation_id = "delete_environment_share"
    )]
    pub async fn delete_environment_share(
        &self,
        environment_share_id: Path<EnvironmentShareId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let record = recorded_http_api_request!(
            "delete_environment_share",
            environment_share_id = environment_share_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .delete_environment_share_internal(environment_share_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn delete_environment_share_internal(
        &self,
        environment_share_id: EnvironmentShareId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<EnvironmentShare>> {
        let actor = AccountId(Uuid::new_v4());
        let share = self
            .environment_share_service
            .delete(&environment_share_id, actor)
            .await?;
        Ok(Json(share))
    }
}
