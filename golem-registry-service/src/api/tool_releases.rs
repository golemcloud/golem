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
use crate::services::tool_release::ToolReleaseService;
use golem_common::model::Page;
use golem_common::model::account::AccountId;
use golem_common::model::tool_release::{ToolRelease, ToolReleaseId};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use std::sync::Arc;
use tracing::Instrument;

pub struct ToolReleasesApi {
    tool_release_service: Arc<ToolReleaseService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1",
    tag = ApiTags::RegistryService,
    tag = ApiTags::ToolReleases
)]
impl ToolReleasesApi {
    pub fn new(
        tool_release_service: Arc<ToolReleaseService>,
        auth_service: Arc<AuthService>,
    ) -> Self {
        Self {
            tool_release_service,
            auth_service,
        }
    }

    /// List tool releases owned by an account
    #[oai(
        path = "/accounts/:account_id/tool-releases",
        method = "get",
        operation_id = "list_account_tool_releases",
        tag = ApiTags::Account
    )]
    async fn list_account_tool_releases(
        &self,
        account_id: Path<AccountId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<ToolRelease>>> {
        let record = recorded_http_api_request!(
            "list_account_tool_releases",
            account_id = account_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(Page {
                values: self
                    .tool_release_service
                    .list_in_account(account_id.0, &auth)
                    .await?,
            }))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// Get an account-owned tool release by ID
    #[oai(
        path = "/tool-releases/:release_id",
        method = "get",
        operation_id = "get_tool_release"
    )]
    async fn get_tool_release(
        &self,
        release_id: Path<ToolReleaseId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ToolRelease>> {
        let record =
            recorded_http_api_request!("get_tool_release", release_id = release_id.0.to_string());
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(
                self.tool_release_service.get(release_id.0, &auth).await?,
            ))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// De-publish an account-owned tool release
    #[oai(
        path = "/tool-releases/:release_id",
        method = "delete",
        operation_id = "de_publish_tool_release"
    )]
    async fn de_publish_tool_release(
        &self,
        release_id: Path<ToolReleaseId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ToolRelease>> {
        let record = recorded_http_api_request!(
            "de_publish_tool_release",
            release_id = release_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(
                self.tool_release_service
                    .de_publish(release_id.0, &auth)
                    .await?,
            ))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }

    /// Restore a de-published account-owned tool release
    #[oai(
        path = "/tool-releases/:release_id/restore",
        method = "post",
        operation_id = "restore_tool_release"
    )]
    async fn restore_tool_release(
        &self,
        release_id: Path<ToolReleaseId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ToolRelease>> {
        let record = recorded_http_api_request!(
            "restore_tool_release",
            release_id = release_id.0.to_string()
        );
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let result = async {
            Ok(Json(
                self.tool_release_service
                    .restore(release_id.0, &auth)
                    .await?,
            ))
        }
        .instrument(record.span.clone())
        .await;
        record.result(result)
    }
}
