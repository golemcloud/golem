use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::project_grant::{
    ProjectGrantError as ProjectGrantServiceError, ProjectGrantService,
};
use crate::service::project_policy::{
    ProjectPolicyError as ProjectPolicyServiceError, ProjectPolicyService,
};
use cloud_common::auth::GolemSecurityScheme;
use cloud_common::model::{ProjectActions, ProjectGrantId, ProjectPolicyId};
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::ProjectId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::{ErrorBody, ErrorsBody};
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

#[derive(ApiResponse, Debug, Clone)]
pub enum ProjectGrantError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Maximum number of projects exceeded
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    /// Project not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for ProjectGrantError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            ProjectGrantError::BadRequest(_) => "BadRequest",
            ProjectGrantError::NotFound(_) => "NotFound",
            ProjectGrantError::LimitExceeded(_) => "LimitExceeded",
            ProjectGrantError::Unauthorized(_) => "Unauthorized",
            ProjectGrantError::InternalError(_) => "InternalError",
        }
    }
}

type Result<T> = std::result::Result<T, ProjectGrantError>;

impl From<AuthServiceError> for ProjectGrantError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                ProjectGrantError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                ProjectGrantError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<ProjectGrantServiceError> for ProjectGrantError {
    fn from(value: ProjectGrantServiceError) -> Self {
        match value {
            ProjectGrantServiceError::Internal(error) => {
                ProjectGrantError::InternalError(Json(ErrorBody { error }))
            }
            ProjectGrantServiceError::Unauthorized(error) => {
                ProjectGrantError::InternalError(Json(ErrorBody { error }))
            }
            ProjectGrantServiceError::ProjectIdNotFound(_) => {
                ProjectGrantError::BadRequest(Json(ErrorsBody {
                    errors: vec!["Project not found".to_string()],
                }))
            }
        }
    }
}

impl From<ProjectPolicyServiceError> for ProjectGrantError {
    fn from(value: ProjectPolicyServiceError) -> Self {
        match value {
            ProjectPolicyServiceError::Internal(error) => {
                ProjectGrantError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

pub struct ProjectGrantApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_grant_service: Arc<dyn ProjectGrantService + Sync + Send>,
    pub project_policy_service: Arc<dyn ProjectPolicyService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/projects", tag = ApiTags::ProjectGrant)]
impl ProjectGrantApi {
    /// Get a project's grants
    ///
    /// Returns all projects grants associated with the given project.
    ///
    /// For each grant:
    /// - `id`` is the identifier of the grant itself
    /// - `granteeAccountId` the account that gets access for the project
    /// - `grantorProjectId` the project ID
    /// - `projectPolicyId` the associated policy - see the project policy API below
    #[oai(
        path = "/:project_id/grants",
        method = "get",
        operation_id = "get_project_grants"
    )]
    async fn get_project_grants(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ProjectGrant>>> {
        let record = recorded_http_api_request!(
            "get_project_grants",
            project_id = project_id.0.to_string(),
        );
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let grants = self
                .project_grant_service
                .get_by_project(&project_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(grants))
        };

        record.result(response)
    }

    /// Get a specific grant of a project
    ///
    /// Returns a specific grant of a specific project. The returned object is the same as the elements of the grants endpoint's response described above.
    #[oai(
        path = "/:project_id/grants/:grant_id",
        method = "get",
        operation_id = "get_project_grant"
    )]
    async fn get_project_grant(
        &self,
        project_id: Path<ProjectId>,
        grant_id: Path<ProjectGrantId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ProjectGrant>> {
        let record = recorded_http_api_request!(
            "get_project_grant",
            project_id = project_id.0.to_string(),
            project_grant_id = grant_id.0.to_string()
        );
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let grant = self
                .project_grant_service
                .get(&project_id.0, &grant_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            match grant {
                Some(grant) => Ok(Json(grant)),
                None => Err(ProjectGrantError::NotFound(Json(ErrorBody {
                    error: "Project grant not found".to_string(),
                }))),
            }
        };

        record.result(response)
    }

    /// Create a project grant
    ///
    /// Creates a new project grant from the following information:
    /// - `granteeAccountId` the account that gets access for the project
    /// - `projectPolicyId` the associated policy - see the project policy API below
    ///
    /// The response describes the new project grant including it's id that can be used to query specifically this grant in the future.
    #[oai(
        path = "/:project_id/grants",
        method = "post",
        operation_id = "create_project_grant"
    )]
    async fn post_project_grant(
        &self,
        project_id: Path<ProjectId>,
        request: Json<ProjectGrantDataRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ProjectGrant>> {
        let record = recorded_http_api_request!(
            "create_project_grant",
            project_id = project_id.0.to_string()
        );
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;

            let data = match request.0.project_policy_id {
                Some(project_policy_id) => ProjectGrantData {
                    grantee_account_id: request.0.grantee_account_id,
                    grantor_project_id: project_id.0,
                    project_policy_id,
                },
                None => {
                    let policy = ProjectPolicy {
                        id: ProjectPolicyId::new_v4(),
                        name: request.0.project_policy_name.unwrap_or("".to_string()),
                        project_actions: ProjectActions {
                            actions: request.0.project_actions.into_iter().collect(),
                        },
                    };

                    self.project_policy_service.create(&policy).await?;

                    ProjectGrantData {
                        grantee_account_id: request.0.grantee_account_id,
                        grantor_project_id: project_id.0,
                        project_policy_id: policy.id,
                    }
                }
            };

            let grant = ProjectGrant {
                id: ProjectGrantId::new_v4(),
                data,
            };

            self.project_grant_service
                .create(&grant, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(grant))
        };

        record.result(response)
    }

    /// Delete a project grant
    ///
    /// Deletes an existing grant of a specific project.
    #[oai(
        path = "/:project_id/grants/:grant_id",
        method = "delete",
        operation_id = "delete_project_grant"
    )]
    async fn delete_project_grant(
        &self,
        project_id: Path<ProjectId>,
        grant_id: Path<ProjectGrantId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteProjectGrantResponse>> {
        let record = recorded_http_api_request!(
            "delete_project_grant",
            project_id = project_id.0.to_string(),
            project_grant_id = grant_id.0.to_string()
        );
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;

            self.project_grant_service
                .delete(&project_id.0, &grant_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(DeleteProjectGrantResponse {}))
        };

        record.result(response)
    }
}
