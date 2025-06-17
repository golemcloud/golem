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
use crate::service::auth::AuthService;
use crate::service::project_policy::ProjectPolicyService;
use golem_common::model::error::ErrorBody;
use golem_common::model::ProjectPolicyId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ProjectPolicyApi {
    pub auth_service: Arc<dyn AuthService>,
    pub project_policy_service: Arc<dyn ProjectPolicyService>,
}

#[OpenApi(prefix_path = "/v1/project-policies", tag = ApiTags::ProjectPolicy)]
impl ProjectPolicyApi {
    /// Get a project policy
    ///
    /// Returns a given project policy by it's ID. Project policy identifiers are used in project grants.
    #[oai(
        path = "/:project_policy_id",
        method = "get",
        operation_id = "get_project_policies"
    )]
    async fn get_project_policies(
        &self,
        project_policy_id: Path<ProjectPolicyId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ProjectPolicy>> {
        let record = recorded_http_api_request!(
            "get_project_policies",
            project_policy_id = project_policy_id.0.to_string(),
        );
        let response = self
            .get_project_policies_internal(project_policy_id.0, token)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_project_policies_internal(
        &self,
        project_policy_id: ProjectPolicyId,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ProjectPolicy>> {
        // FIXME auth check
        let _ = self.auth_service.authorization(token.as_ref()).await?;
        let policy = self.project_policy_service.get(&project_policy_id).await?;
        match policy {
            Some(policy) => Ok(Json(policy)),
            None => Err(ApiError::NotFound(Json(ErrorBody {
                error: "Project policy not found".to_string(),
            }))),
        }
    }

    /// Create a project policy
    ///
    /// Creates a new project policy and returns the object describing it, including the newly created policy's id.
    #[oai(path = "/", method = "post", operation_id = "create_project_policy")]
    async fn post_project_policy(
        &self,
        request: Json<ProjectPolicyData>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ProjectPolicy>> {
        let record = recorded_http_api_request!(
            "create_project_policy",
            project_policy_name = request.0.name.to_string(),
        );
        let response = self
            .post_project_policy_internal(request.0, token)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn post_project_policy_internal(
        &self,
        request: ProjectPolicyData,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<ProjectPolicy>> {
        // FIXME auth check
        let _ = self.auth_service.authorization(token.as_ref()).await?;

        let policy = ProjectPolicy {
            id: ProjectPolicyId::new_v4(),
            name: request.name,
            project_actions: request.project_actions,
        };
        self.project_policy_service.create(&policy).await?;

        Ok(Json(policy))
    }
}
