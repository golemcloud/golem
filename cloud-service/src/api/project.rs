use std::sync::Arc;

use cloud_common::auth::GolemSecurityScheme;
use golem_common::model::ProjectId;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;

use crate::api::ApiTags;
use crate::model::*;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::project::{ProjectError as ProjectServiceError, ProjectService};
use crate::service::project_auth::{ProjectAuthorisationError, ProjectAuthorisationService};
use cloud_common::model::ProjectAction;
use golem_service_base::model::{ErrorBody, ErrorsBody};

#[derive(ApiResponse)]
pub enum ProjectError {
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
    /// Project already exists
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

type Result<T> = std::result::Result<T, ProjectError>;

impl From<AuthServiceError> for ProjectError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => {
                ProjectError::Unauthorized(Json(ErrorBody { error }))
            }
            AuthServiceError::Unexpected(error) => {
                ProjectError::InternalError(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<ProjectServiceError> for ProjectError {
    fn from(value: ProjectServiceError) -> Self {
        match value {
            ProjectServiceError::Internal(error) => {
                ProjectError::InternalError(Json(ErrorBody { error }))
            }
            ProjectServiceError::Unauthorized(error) => {
                ProjectError::Unauthorized(Json(ErrorBody { error }))
            }
            ProjectServiceError::LimitExceeded(error) => {
                ProjectError::LimitExceeded(Json(ErrorBody { error }))
            }
        }
    }
}

impl From<ProjectAuthorisationError> for ProjectError {
    fn from(value: ProjectAuthorisationError) -> Self {
        match value {
            ProjectAuthorisationError::Internal(error) => {
                ProjectError::InternalError(Json(ErrorBody { error }))
            }
            ProjectAuthorisationError::Unauthorized(error) => {
                ProjectError::Unauthorized(Json(ErrorBody { error }))
            }
        }
    }
}

pub struct ProjectApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_service: Arc<dyn ProjectService + Sync + Send>,
    pub project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v2/projects", tag = ApiTags::Project)]
impl ProjectApi {
    /// Get the default project
    ///
    /// - name of the project can be used for lookup the project if the ID is now known
    /// - defaultEnvironmentId is currently always default
    /// - projectType is either Default
    #[oai(path = "/default", method = "get")]
    async fn get_default_project(&self, token: GolemSecurityScheme) -> Result<Json<Project>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let project = self.project_service.get_own_default(&auth).await?;
        Ok(Json(project))
    }

    /// List all projects
    ///
    /// Returns all projects of the account if no project-name is specified.
    /// Otherwise, returns all projects of the account that has the given name.
    /// As unique names are not enforced on the API level, the response may contain multiple entries.
    #[oai(path = "/", method = "get")]
    async fn get_projects(
        &self,
        /// Filter by project name
        #[oai(name = "project-name")]
        project_name: Query<Option<String>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<Project>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        match project_name.0 {
            Some(project_name) => {
                let projects = self
                    .project_service
                    .get_own_by_name(&project_name, &auth)
                    .await?;
                Ok(Json(projects))
            }
            None => {
                let projects = self.project_service.get_own(&auth).await?;
                Ok(Json(projects))
            }
        }
    }

    /// Create project
    ///
    /// Creates a new project. The ownerAccountId must be the caller's account ID.
    #[oai(path = "/", method = "post")]
    async fn post_project(
        &self,
        request: Json<ProjectDataRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Project>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;

        let project = Project {
            project_id: ProjectId::new_v4(),
            project_data: ProjectData {
                name: request.0.name,
                owner_account_id: request.0.owner_account_id,
                description: request.0.description,
                default_environment_id: "default".to_string(),
                project_type: ProjectType::NonDefault,
            },
        };

        self.project_service.create(&project, &auth).await?;
        Ok(Json(project))
    }

    /// Get project by ID
    ///
    /// Gets a project by its identifier. Response is the same as for the default project.
    #[oai(path = "/:project_id", method = "get")]
    async fn get_project(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Project>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let project = self.project_service.get(&project_id.0, &auth).await?;
        match project {
            Some(p) => Ok(Json(p)),
            None => Err(ProjectError::NotFound(Json(ErrorBody {
                error: "Project not found".to_string(),
            }))),
        }
    }

    /// Delete project
    ///
    /// Deletes a project given by its identifier.
    #[oai(path = "/:project_id", method = "delete")]
    async fn delete_project(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<DeleteProjectResponse>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        self.project_service.delete(&project_id.0, &auth).await?;
        Ok(Json(DeleteProjectResponse {}))
    }

    /// Get project actions
    ///
    /// Returns a list of actions that can be performed on the project.
    #[oai(path = "/:project_id/actions", method = "get")]
    async fn get_project_actions(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ProjectAction>>> {
        let auth = self.auth_service.authorization(token.as_ref()).await?;
        let result = self
            .project_auth_service
            .get_by_project(&project_id.0, &auth)
            .await?;
        Ok(Json(Vec::from_iter(result.actions.actions)))
    }
}
