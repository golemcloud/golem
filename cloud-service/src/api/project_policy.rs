use crate::api::{ApiError, ApiResult, ApiTags};
use crate::model::*;
use crate::service::auth::AuthService;
use crate::service::project_policy::ProjectPolicyService;
use cloud_common::auth::GolemSecurityScheme;
use cloud_common::model::ProjectPolicyId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::ErrorBody;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ProjectPolicyApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
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
        let response = {
            // FIXME auth check
            let _ = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let policy = self
                .project_policy_service
                .get(&project_policy_id.0)
                .instrument(record.span.clone())
                .await?;
            match policy {
                Some(policy) => Ok(Json(policy)),
                None => Err(ApiError::NotFound(Json(ErrorBody {
                    error: "Project policy not found".to_string(),
                }))),
            }
        };

        record.result(response)
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
        let response = {
            // FIXME auth check
            let _ = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;

            let policy = ProjectPolicy {
                id: ProjectPolicyId::new_v4(),
                name: request.0.name,
                project_actions: request.0.project_actions,
            };
            self.project_policy_service
                .create(&policy)
                .instrument(record.span.clone())
                .await?;

            Ok(Json(policy))
        };

        record.result(response)
    }
}
