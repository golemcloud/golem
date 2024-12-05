use crate::api::{ApiTags, LimitedApiError, LimitedApiResult};
use crate::model::*;
use crate::service::auth::AuthService;
use crate::service::project::ProjectService;
use crate::service::project_auth::ProjectAuthorisationService;
use cloud_common::auth::GolemSecurityScheme;
use cloud_common::model::ProjectAction;
use golem_common::model::plugin::{
    PluginInstallation, PluginInstallationCreation, PluginInstallationUpdate,
};
use golem_common::model::{Empty, PluginInstallationId, ProjectId};
use golem_common::recorded_http_api_request;
use golem_service_base::model::ErrorBody;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ProjectApi {
    pub auth_service: Arc<dyn AuthService + Sync + Send>,
    pub project_service: Arc<dyn ProjectService + Sync + Send>,
    pub project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/projects", tag = ApiTags::Project)]
impl ProjectApi {
    /// Get the default project
    ///
    /// - name of the project can be used for lookup the project if the ID is now known
    /// - defaultEnvironmentId is currently always default
    /// - projectType is either Default
    #[oai(
        path = "/default",
        method = "get",
        operation_id = "get_default_project"
    )]
    async fn get_default_project(
        &self,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Project>> {
        let record = recorded_http_api_request!("get_default_project",);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let project = self
                .project_service
                .get_own_default(&auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(project))
        };

        record.result(response)
    }

    /// List all projects
    ///
    /// Returns all projects of the account if no project-name is specified.
    /// Otherwise, returns all projects of the account that has the given name.
    /// As unique names are not enforced on the API level, the response may contain multiple entries.
    #[oai(path = "/", method = "get", operation_id = "get_projects")]
    async fn get_projects(
        &self,
        /// Filter by project name
        #[oai(name = "project-name")]
        project_name: Query<Option<String>>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Vec<Project>>> {
        let record =
            recorded_http_api_request!("get_projects", project_name = project_name.0.as_ref(),);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;

            match project_name.0 {
                Some(project_name) => {
                    let projects = self
                        .project_service
                        .get_own_by_name(&project_name, &auth)
                        .instrument(record.span.clone())
                        .await?;
                    Ok(Json(projects))
                }
                None => {
                    let projects = self
                        .project_service
                        .get_own(&auth)
                        .instrument(record.span.clone())
                        .await?;
                    Ok(Json(projects))
                }
            }
        };

        record.result(response)
    }

    /// Create project
    ///
    /// Creates a new project. The ownerAccountId must be the caller's account ID.
    #[oai(path = "/", method = "post", operation_id = "create_project")]
    async fn create_project(
        &self,
        request: Json<ProjectDataRequest>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Project>> {
        let record = recorded_http_api_request!("create_project", project_name = request.0.name,);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;

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

            self.project_service
                .create(&project, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(project))
        };

        record.result(response)
    }

    /// Get project by ID
    ///
    /// Gets a project by its identifier. Response is the same as for the default project.
    #[oai(path = "/:project_id", method = "get", operation_id = "get_project")]
    async fn get_project(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Project>> {
        let record =
            recorded_http_api_request!("get_project", project_id = project_id.0.to_string(),);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let project = self
                .project_service
                .get(&project_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            match project {
                Some(p) => Ok(Json(p)),
                None => Err(LimitedApiError::NotFound(Json(ErrorBody {
                    error: "Project not found".to_string(),
                }))),
            }
        };

        record.result(response)
    }

    /// Delete project
    ///
    /// Deletes a project given by its identifier.
    #[oai(
        path = "/:project_id",
        method = "delete",
        operation_id = "delete_project"
    )]
    async fn delete_project(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<DeleteProjectResponse>> {
        let record =
            recorded_http_api_request!("delete_project", project_id = project_id.0.to_string(),);
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            self.project_service
                .delete(&project_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(DeleteProjectResponse {}))
        };

        record.result(response)
    }

    /// Get project actions
    ///
    /// Returns a list of actions that can be performed on the project.
    #[oai(
        path = "/:project_id/actions",
        method = "get",
        operation_id = "get_project_actions"
    )]
    async fn get_project_actions(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Vec<ProjectAction>>> {
        let record = recorded_http_api_request!(
            "get_project_actions",
            project_id = project_id.0.to_string(),
        );
        let response = {
            let auth = self
                .auth_service
                .authorization(token.as_ref())
                .instrument(record.span.clone())
                .await?;
            let result = self
                .project_auth_service
                .get_by_project(&project_id.0, &auth)
                .instrument(record.span.clone())
                .await?;
            Ok(Json(Vec::from_iter(result.actions.actions)))
        };

        record.result(response)
    }

    /// Gets the list of plugins installed for the given project
    #[oai(
        path = "/:project_id/plugins/installs",
        method = "get",
        operation_id = "get_installed_plugins"
    )]
    async fn get_installed_plugins(
        &self,
        project_id: Path<ProjectId>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Vec<PluginInstallation>>> {
        let record = recorded_http_api_request!(
            "get_installed_plugins",
            project_id = project_id.0.to_string(),
        );
        let auth = self
            .auth_service
            .authorization(token.as_ref())
            .instrument(record.span.clone())
            .await?;

        let response = self
            .project_service
            .get_plugin_installations_for_project(&project_id, &auth)
            .await
            .map_err(|e| e.into())
            .map(Json);

        record.result(response)
    }

    /// Installs a new plugin for this project
    #[oai(
        path = "/:project_id/plugins/installs",
        method = "post",
        operation_id = "install_plugin_to_project"
    )]
    async fn install_plugin(
        &self,
        project_id: Path<ProjectId>,
        plugin: Json<PluginInstallationCreation>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<PluginInstallation>> {
        let record = recorded_http_api_request!(
            "install_plugin",
            project_id = project_id.0.to_string(),
            plugin_name = plugin.name.clone(),
            plugin_version = plugin.version.clone()
        );
        let auth = self
            .auth_service
            .authorization(token.as_ref())
            .instrument(record.span.clone())
            .await?;

        let response = self
            .project_service
            .create_plugin_installation_for_project(&project_id, plugin.0, &auth)
            .await
            .map_err(|e| e.into())
            .map(Json);

        record.result(response)
    }

    /// Updates the priority or parameters of a plugin installation
    #[oai(
        path = "/:project_id/plugins/installs/:installation_id",
        method = "put",
        operation_id = "update_installed_plugin_in_project"
    )]
    async fn update_installed_plugin(
        &self,
        project_id: Path<ProjectId>,
        installation_id: Path<PluginInstallationId>,
        update: Json<PluginInstallationUpdate>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Empty>> {
        let record = recorded_http_api_request!(
            "update_installed_plugin",
            project_id = project_id.0.to_string(),
            installation_id = installation_id.0.to_string()
        );
        let auth = self
            .auth_service
            .authorization(token.as_ref())
            .instrument(record.span.clone())
            .await?;

        let response = self
            .project_service
            .update_plugin_installation_for_project(
                &project_id.0,
                &installation_id.0,
                update.0,
                &auth,
            )
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }

    /// Uninstalls a plugin from this project
    #[oai(
        path = "/:project_id/latest/plugins/installs/:installation_id",
        method = "delete",
        operation_id = "uninstall_plugin_from_project"
    )]
    async fn uninstall_plugin(
        &self,
        project_id: Path<ProjectId>,
        installation_id: Path<PluginInstallationId>,
        token: GolemSecurityScheme,
    ) -> LimitedApiResult<Json<Empty>> {
        let record = recorded_http_api_request!(
            "uninstall_plugin",
            project_id = project_id.0.to_string(),
            installation_id = installation_id.0.to_string()
        );
        let auth = self
            .auth_service
            .authorization(token.as_ref())
            .instrument(record.span.clone())
            .await?;

        let response = self
            .project_service
            .delete_plugin_installation_for_project(&installation_id.0, &project_id.0, &auth)
            .await
            .map_err(|e| e.into())
            .map(|_| Json(Empty {}));

        record.result(response)
    }
}
