use crate::service::CloudComponentError;
use async_trait::async_trait;
use bytes::Bytes;
use cloud_common::auth::CloudAuthCtx;
use cloud_common::clients::auth::BaseAuthService;
use cloud_common::clients::limit::LimitService;
use cloud_common::clients::project::ProjectService;
use cloud_common::model::{
    CloudComponentOwner, CloudPluginOwner, CloudPluginScope, ComponentOwnershipQuery, ProjectAction,
};
use futures_util::stream::BoxStream;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::component_constraint::FunctionConstraints;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::{
    PluginInstallation, PluginInstallationAction, PluginInstallationCreation,
    PluginInstallationUpdate,
};
use golem_common::model::{ComponentId, ComponentType, ComponentVersion, PluginInstallationId};
use golem_common::model::{InitialComponentFile, ProjectId};
use golem_common::SafeDisplay;
use golem_component_service_base::model::{
    Component, ComponentConstraints, InitialComponentFilesArchiveAndPermissions,
};
use golem_component_service_base::service::component::{
    ComponentByNameAndVersion, ComponentError, ComponentService,
};
use golem_component_service_base::service::plugin::{PluginError, PluginService};
use golem_service_base::model::*;
use std::collections::HashMap;
use std::sync::Arc;

pub struct CloudComponentService {
    base_component_service: Arc<dyn ComponentService<CloudComponentOwner> + Sync + Send>,
    auth_service: Arc<dyn BaseAuthService + Sync + Send>,
    limit_service: Arc<dyn LimitService + Sync + Send>,
    project_service: Arc<dyn ProjectService + Sync + Send>,
    plugin_service: Arc<dyn PluginService<CloudPluginOwner, CloudPluginScope> + Sync + Send>,
}

impl CloudComponentService {
    pub fn new(
        base_component_service: Arc<dyn ComponentService<CloudComponentOwner> + Sync + Send>,
        auth_service: Arc<dyn BaseAuthService + Sync + Send>,
        limit_service: Arc<dyn LimitService + Sync + Send>,
        project_service: Arc<dyn ProjectService + Sync + Send>,
        plugin_service: Arc<dyn PluginService<CloudPluginOwner, CloudPluginScope> + Sync + Send>,
    ) -> Self {
        CloudComponentService {
            base_component_service,
            auth_service,
            limit_service,
            project_service,
            plugin_service,
        }
    }

    async fn get_owner(
        &self,
        project_id: Option<ProjectId>,
        auth: &CloudAuthCtx,
    ) -> Result<CloudComponentOwner, CloudComponentError> {
        if let Some(project_id) = project_id.clone() {
            Ok(self
                .is_authorized_by_project(auth, &project_id, &ProjectAction::ViewComponent)
                .await?)
        } else {
            let project = self.project_service.get_default(&auth.token_secret).await?;
            Ok(CloudComponentOwner {
                account_id: project.owner_account_id,
                project_id: project.id,
            })
        }
    }

    pub async fn create(
        &self,
        project_id: Option<ProjectId>,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        auth: &CloudAuthCtx,
        env: HashMap<String, String>,
    ) -> Result<Component<CloudComponentOwner>, CloudComponentError> {
        let component_id = ComponentId::new_v4();

        let owner = self.get_owner(project_id, auth).await?;

        self.base_component_service
            .find_id_by_name(component_name, &owner)
            .await?
            .map_or(Ok(()), |id| {
                Err(CloudComponentError::BaseComponentError(
                    ComponentError::AlreadyExists(id),
                ))
            })?;

        let component_size: u64 = data.len() as u64;

        self.limit_service
            .update_component_limit(&owner.account_id, &component_id, 1, component_size as i64)
            .await?;

        let component = self
            .base_component_service
            .create(
                &component_id,
                component_name,
                component_type,
                data.clone(),
                files,
                vec![],
                dynamic_linking,
                &owner,
                env,
            )
            .await?;

        Ok(component)
    }

    pub async fn create_internal(
        &self,
        project_id: Option<ProjectId>,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        files: Vec<InitialComponentFile>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        auth: &CloudAuthCtx,
        env: HashMap<String, String>,
    ) -> Result<Component<CloudComponentOwner>, CloudComponentError> {
        let component_id = ComponentId::new_v4();

        let owner = self.get_owner(project_id, auth).await?;

        self.base_component_service
            .find_id_by_name(component_name, &owner)
            .await?
            .map_or(Ok(()), |id| {
                Err(CloudComponentError::BaseComponentError(
                    ComponentError::AlreadyExists(id),
                ))
            })?;

        let component_size: u64 = data.len() as u64;

        self.limit_service
            .update_component_limit(&owner.account_id, &component_id, 1, component_size as i64)
            .await?;

        let component = self
            .base_component_service
            .create_internal(
                &component_id,
                component_name,
                component_type,
                data.clone(),
                files,
                vec![],
                dynamic_linking,
                &owner,
                env,
            )
            .await?;

        Ok(component)
    }

    pub async fn update(
        &self,
        component_id: &ComponentId,
        component_type: Option<ComponentType>,
        data: Vec<u8>,
        files: Option<InitialComponentFilesArchiveAndPermissions>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        auth: &CloudAuthCtx,
        env: HashMap<String, String>,
    ) -> Result<Component<CloudComponentOwner>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;

        let component_size: u64 = data.len() as u64;

        self.limit_service
            .update_component_limit(&owner.account_id, component_id, 0, component_size as i64)
            .await?;

        let component = self
            .base_component_service
            .update(
                component_id,
                data.clone(),
                component_type,
                files,
                dynamic_linking,
                &owner,
                env,
            )
            .await?;

        Ok(component)
    }

    pub async fn update_internal(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        component_type: Option<ComponentType>,
        files: Option<Vec<InitialComponentFile>>,
        dynamic_linking: HashMap<String, DynamicLinkedInstance>,
        auth: &CloudAuthCtx,
        env: HashMap<String, String>,
    ) -> Result<Component<CloudComponentOwner>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;

        let component_size: u64 = data.len() as u64;

        self.limit_service
            .update_component_limit(&owner.account_id, component_id, 0, component_size as i64)
            .await?;

        let component = self
            .base_component_service
            .update_internal(
                component_id,
                data.clone(),
                component_type,
                files,
                dynamic_linking,
                &owner,
                env,
            )
            .await?;

        Ok(component)
    }

    pub async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<u8>, CloudComponentError> {
        let namespace = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;

        let data = self
            .base_component_service
            .download(component_id, version, &namespace)
            .await?;

        Ok(data)
    }

    pub async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;

        let stream = self
            .base_component_service
            .download_stream(component_id, version, &owner)
            .await?;
        Ok(stream)
    }

    pub async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        component_name: Option<ComponentName>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<Component<CloudComponentOwner>>, CloudComponentError> {
        let owner = self.get_owner(project_id, auth).await?;

        let result = self
            .base_component_service
            .find_by_name(component_name, &owner)
            .await?;

        Ok(result)
    }

    pub async fn find_by_project_and_names(
        &self,
        project_id: Option<ProjectId>,
        component_names: Vec<ComponentByNameAndVersion>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<Component<CloudComponentOwner>>, CloudComponentError> {
        let owner = self.get_owner(project_id, auth).await?;

        let result = self
            .base_component_service
            .find_by_names(component_names, &owner)
            .await?;

        Ok(result)
    }

    pub async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<Component<CloudComponentOwner>>, CloudComponentError> {
        let owner = self
            .is_authorized_by_project(auth, project_id, &ProjectAction::ViewComponent)
            .await?;

        let result = self
            .base_component_service
            .find_by_name(None, &owner)
            .await?;
        Ok(result)
    }

    pub async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Option<Component<CloudComponentOwner>>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(
                auth,
                &component_id.component_id,
                &ProjectAction::ViewComponent,
            )
            .await?;

        let result = self
            .base_component_service
            .get_by_version(component_id, &owner)
            .await?;

        Ok(result)
    }

    pub async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Option<Component<CloudComponentOwner>>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let result = self
            .base_component_service
            .get_latest_version(component_id, &owner)
            .await?;
        Ok(result)
    }

    pub async fn get(
        &self,
        component_id: &ComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<Component<CloudComponentOwner>>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let result = self
            .base_component_service
            .get(component_id, &owner)
            .await?;

        Ok(result)
    }

    pub async fn create_or_update_constraint(
        &self,
        component_id: ComponentId,
        constraints: FunctionConstraints,
        auth: &CloudAuthCtx,
    ) -> Result<ComponentConstraints<CloudComponentOwner>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, &component_id, &ProjectAction::UpdateComponent)
            .await?;

        let component_constraints = ComponentConstraints {
            owner,
            component_id,
            constraints,
        };

        let result = self
            .base_component_service
            .create_or_update_constraint(&component_constraints)
            .await?;

        Ok(result)
    }

    pub async fn delete_constraints(
        &self,
        component_id: ComponentId,
        constraints: FunctionConstraints,
        auth: &CloudAuthCtx,
    ) -> Result<ComponentConstraints<CloudComponentOwner>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, &component_id, &ProjectAction::UpdateComponent)
            .await?;

        let constraints = ComponentConstraints {
            owner: owner.clone(),
            component_id,
            constraints,
        };

        let result = self
            .base_component_service
            .delete_constraints(
                &owner,
                &constraints.component_id,
                &constraints.function_signatures(),
            )
            .await?;

        Ok(result)
    }

    pub async fn get_plugin_installations_for_component(
        &self,
        auth: &CloudAuthCtx,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(CloudPluginOwner, Vec<PluginInstallation>), PluginError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;
        let installations = self
            .base_component_service
            .get_plugin_installations_for_component(&owner, component_id, component_version)
            .await?;

        Ok((owner.into(), installations))
    }

    pub async fn create_plugin_installation_for_component(
        &self,
        auth: &CloudAuthCtx,
        component_id: &ComponentId,
        installation: PluginInstallationCreation,
    ) -> Result<(CloudPluginOwner, PluginInstallation), PluginError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;
        let plugin_owner: CloudPluginOwner = owner.clone().into();

        let plugin_definition = self
            .plugin_service
            .get(&plugin_owner, &installation.name, &installation.version)
            .await?;

        if let Some(plugin_definition) = plugin_definition {
            if plugin_definition
                .scope
                .valid_in_component(component_id, &owner.project_id)
            {
                let installation = self
                    .base_component_service
                    .create_plugin_installation_for_component(&owner, component_id, installation)
                    .await?;

                Ok((owner.into(), installation))
            } else {
                Err(PluginError::InvalidScope {
                    plugin_name: installation.name.clone(),
                    plugin_version: installation.version.clone(),
                    details: format!("not available for component {}", component_id.0),
                })
            }
        } else {
            Err(PluginError::PluginNotFound {
                plugin_name: installation.name.clone(),
                plugin_version: installation.version.clone(),
            })
        }
    }

    pub async fn update_plugin_installation_for_component(
        &self,
        auth: &CloudAuthCtx,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), PluginError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;
        self.base_component_service
            .update_plugin_installation_for_component(&owner, installation_id, component_id, update)
            .await
    }

    pub async fn delete_plugin_installation_for_component(
        &self,
        auth: &CloudAuthCtx,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), PluginError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;
        self.base_component_service
            .delete_plugin_installation_for_component(&owner, installation_id, component_id)
            .await
    }

    pub async fn batch_update_plugin_installations_for_component(
        &self,
        auth: &CloudAuthCtx,
        component_id: &ComponentId,
        actions: &[PluginInstallationAction],
    ) -> Result<Vec<Option<PluginInstallation>>, PluginError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;
        self.base_component_service
            .batch_update_plugin_installations_for_component(&owner, component_id, actions)
            .await
    }

    pub async fn get_file_contents(
        &self,
        auth: &CloudAuthCtx,
        component_id: &ComponentId,
        version: ComponentVersion,
        path: &str,
    ) -> Result<BoxStream<'static, Result<Bytes, ComponentError>>, CloudComponentError> {
        let owner = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let result = self
            .base_component_service
            .get_file_contents(component_id, version, path, &owner)
            .await?;
        Ok(result)
    }

    async fn is_authorized_by_component(
        &self,
        auth: &CloudAuthCtx,
        component_id: &ComponentId,
        action: &ProjectAction,
    ) -> Result<CloudComponentOwner, CloudComponentError> {
        let owner = self.base_component_service.get_owner(component_id).await?;

        match owner {
            Some(owner) => {
                self.is_authorized_by_project(auth, &owner.project_id, action)
                    .await
            }
            None => Err(CloudComponentError::Unauthorized(format!(
                "Account unauthorized to perform action on component {}: {}",
                component_id.0, action
            ))),
        }
    }

    async fn is_authorized_by_project(
        &self,
        auth: &CloudAuthCtx,
        project_id: &ProjectId,
        action: &ProjectAction,
    ) -> Result<CloudComponentOwner, CloudComponentError> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, action.clone(), auth)
            .await?;

        Ok(CloudComponentOwner {
            account_id: namespace.account_id,
            project_id: namespace.project_id,
        })
    }
}

#[async_trait]
impl ComponentOwnershipQuery for CloudComponentService {
    async fn get_project(
        &self,
        component_id: &ComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Option<ProjectId>, String> {
        let component = self
            .get_latest_version(component_id, auth)
            .await
            .map_err(|err| err.to_safe_string())?;
        Ok(component.map(|component| component.owner.project_id.clone()))
    }
}
