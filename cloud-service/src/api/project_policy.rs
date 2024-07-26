use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::project_policy::{
    ProjectPolicyError as ProjectPolicyServiceError, ProjectPolicyService,
};
use cloud_common::auth::GolemSecurityScheme;
use cloud_common::model::ProjectPolicyId;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::recorded_http_api_request;
use golem_service_base::model::{ErrorBody, ErrorsBody};
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

#[derive(ApiResponse, Debug, Clone)]
pub enum ProjectPolicyError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Project not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for ProjectPolicyError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            ProjectPolicyError::BadRequest(_) => "BadRequest",
            ProjectPolicyError::NotFound(_) => "NotFound",
            ProjectPolicyError::Unauthorized(_) => "Unauthorized",
            ProjectPolicyError::InternalError(_) => "InternalError",
        }
    }
}

type Result<T> = std::result::Result<T, ProjectPolicyError>;

impl From<AuthServiceError> for ProjectPolicyError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                ProjectPolicyError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                ProjectPolicyError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<ProjectPolicyServiceError> for ProjectPolicyError {
    fn from(value: ProjectPolicyServiceError) -> Self {
        match value {
            ProjectPolicyServiceError::Internal(error) => {
                ProjectPolicyError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

pub struct ProjectPolicyApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/project-policies", tag = ApiTags::ProjectPolicy)]
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
    ) -> Result<Json<ProjectPolicy>> {
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
                None => Err(ProjectPolicyError::NotFound(Json(ErrorBody {
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
    ) -> Result<Json<ProjectPolicy>> {
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
