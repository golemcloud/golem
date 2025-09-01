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

use crate::authed::{
    is_authorized_by_component, is_authorized_by_project, is_authorized_by_project_or_default,
};
use crate::error::ComponentError;
use crate::model::InitialComponentFilesArchiveAndPermissions;
use crate::model::{Component, ComponentByNameAndVersion, ComponentConstraints};
use crate::service::component::ComponentService;
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_common::model::agent::AgentType;
use golem_common::model::auth::AuthCtx;
use golem_common::model::auth::ProjectAction;
use golem_common::model::component::VersionedComponentId;
use golem_common::model::component_constraint::FunctionConstraints;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::plugin::{
    PluginInstallation, PluginInstallationAction, PluginInstallationCreation,
    PluginInstallationUpdate,
};
use golem_common::model::{ComponentId, ComponentType, ComponentVersion, PluginInstallationId};
use golem_common::model::{InitialComponentFile, ProjectId};
use golem_service_base::clients::auth::AuthService;
use golem_service_base::clients::project::ProjectService;
use golem_service_base::model::ComponentName;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::vec;

pub struct AuthedComponentService {
    component_service: Arc<dyn ComponentService>,
    auth_service: Arc<AuthService>,
    project_service: Arc<dyn ProjectService>,
}

impl AuthedComponentService {
    pub fn new(
        base_component_service: Arc<dyn ComponentService>,
        auth_service: Arc<AuthService>,
        project_service: Arc<dyn ProjectService>,
    ) -> Self {
        Self {
            component_service: base_component_service,
            auth_service,
            project_service,
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
        auth: &AuthCtx,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let component_id = ComponentId::new_v4();
        let owner = is_authorized_by_project_or_default(
            &self.auth_service,
            &self.project_service,
            auth,
            project_id,
            &ProjectAction::CreateComponent,
        )
        .await?;

        let component = self
            .component_service
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
                agent_types,
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
        auth: &AuthCtx,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let component_id = ComponentId::new_v4();
        let owner = is_authorized_by_project_or_default(
            &self.auth_service,
            &self.project_service,
            auth,
            project_id,
            &ProjectAction::CreateComponent,
        )
        .await?;

        let component = self
            .component_service
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
                agent_types,
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
        auth: &AuthCtx,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        let component = self
            .component_service
            .update(
                component_id,
                data.clone(),
                component_type,
                files,
                dynamic_linking,
                &owner,
                env,
                agent_types,
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
        auth: &AuthCtx,
        env: HashMap<String, String>,
        agent_types: Vec<AgentType>,
    ) -> Result<Component, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        let component = self
            .component_service
            .update_internal(
                component_id,
                data.clone(),
                component_type,
                files,
                dynamic_linking,
                &owner,
                env,
                agent_types,
            )
            .await?;

        Ok(component)
    }

    pub async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AuthCtx,
    ) -> Result<Vec<u8>, ComponentError> {
        let namespace = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let data = self
            .component_service
            .download(component_id, version, &namespace)
            .await?;

        Ok(data)
    }

    pub async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AuthCtx,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let stream = self
            .component_service
            .download_stream(component_id, version, &owner)
            .await?;
        Ok(stream)
    }

    pub async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        component_name: Option<ComponentName>,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        let owner = is_authorized_by_project_or_default(
            &self.auth_service,
            &self.project_service,
            auth,
            project_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let result = self
            .component_service
            .find_by_name(component_name, &owner)
            .await?;

        Ok(result)
    }

    pub async fn find_by_project_and_names(
        &self,
        project_id: Option<ProjectId>,
        component_names: Vec<ComponentByNameAndVersion>,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        let owner = is_authorized_by_project_or_default(
            &self.auth_service,
            &self.project_service,
            auth,
            project_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let result = self
            .component_service
            .find_by_names(component_names, &owner)
            .await?;

        Ok(result)
    }

    pub async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        let owner = is_authorized_by_project(
            &self.auth_service,
            auth,
            project_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let result = self.component_service.find_by_name(None, &owner).await?;
        Ok(result)
    }

    pub async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        auth: &AuthCtx,
    ) -> Result<Option<Component>, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            &component_id.component_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let result = self
            .component_service
            .get_by_version(component_id, &owner)
            .await?;

        Ok(result)
    }

    pub async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        auth: &AuthCtx,
    ) -> Result<Option<Component>, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let result = self
            .component_service
            .get_latest_version(component_id, &owner)
            .await?;
        Ok(result)
    }

    pub async fn get(
        &self,
        component_id: &ComponentId,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        let result = self.component_service.get(component_id, &owner).await?;
        Ok(result)
    }

    pub async fn create_or_update_constraint(
        &self,
        component_id: ComponentId,
        constraints: FunctionConstraints,
        auth: &AuthCtx,
    ) -> Result<ComponentConstraints, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            &component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        let component_constraints = ComponentConstraints {
            owner,
            component_id,
            constraints,
        };

        let result = self
            .component_service
            .create_or_update_constraint(&component_constraints)
            .await?;

        Ok(result)
    }

    pub async fn delete_constraints(
        &self,
        component_id: ComponentId,
        constraints: FunctionConstraints,
        auth: &AuthCtx,
    ) -> Result<ComponentConstraints, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            &component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        let constraints = ComponentConstraints {
            owner: owner.clone(),
            component_id,
            constraints,
        };

        let result = self
            .component_service
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
        auth: &AuthCtx,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Vec<PluginInstallation>, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        let installations = self
            .component_service
            .get_plugin_installations_for_component(&owner, component_id, component_version)
            .await?;

        Ok(installations)
    }

    pub async fn create_plugin_installation_for_component(
        &self,
        auth: &AuthCtx,
        component_id: &ComponentId,
        installation: PluginInstallationCreation,
    ) -> Result<PluginInstallation, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        let installation = self
            .component_service
            .create_plugin_installation_for_component(&owner, component_id, installation)
            .await?;

        Ok(installation)
    }

    pub async fn update_plugin_installation_for_component(
        &self,
        auth: &AuthCtx,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
        update: PluginInstallationUpdate,
    ) -> Result<(), ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        self.component_service
            .update_plugin_installation_for_component(&owner, installation_id, component_id, update)
            .await
    }

    pub async fn delete_plugin_installation_for_component(
        &self,
        auth: &AuthCtx,
        installation_id: &PluginInstallationId,
        component_id: &ComponentId,
    ) -> Result<(), ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        self.component_service
            .delete_plugin_installation_for_component(&owner, installation_id, component_id)
            .await
    }

    pub async fn batch_update_plugin_installations_for_component(
        &self,
        auth: &AuthCtx,
        component_id: &ComponentId,
        actions: &[PluginInstallationAction],
    ) -> Result<Vec<Option<PluginInstallation>>, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::UpdateComponent,
        )
        .await?;

        self.component_service
            .batch_update_plugin_installations_for_component(&owner, component_id, actions)
            .await
    }

    pub async fn get_file_contents(
        &self,
        auth: &AuthCtx,
        component_id: &ComponentId,
        version: ComponentVersion,
        path: &str,
    ) -> Result<BoxStream<'static, Result<Bytes, ComponentError>>, ComponentError> {
        let owner = is_authorized_by_component(
            &self.auth_service,
            &self.component_service,
            auth,
            component_id,
            &ProjectAction::ViewComponent,
        )
        .await?;

        self.component_service
            .get_file_contents(component_id, version, path, &owner)
            .await
    }
}

impl Debug for AuthedComponentService {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentServiceDefault").finish()
    }
}
