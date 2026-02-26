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

mod error;
mod utils;
mod write;

pub use self::error::ComponentError;
pub use self::write::ComponentWriteService;
use super::component_object_store::ComponentObjectStore;
use super::deployment::DeploymentService;
use super::environment::EnvironmentService;
use crate::repo::component::ComponentRepo;
use crate::services::deployment::DeploymentError;
use crate::services::environment::EnvironmentError;
use futures::stream::BoxStream;
use golem_common::model::component::ComponentId;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_service_base::model::Component;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::EnvironmentAction;
use std::sync::Arc;
use tracing::info;

pub struct ComponentService {
    component_repo: Arc<dyn ComponentRepo>,
    object_store: Arc<ComponentObjectStore>,
    environment_service: Arc<EnvironmentService>,
    deployment_service: Arc<DeploymentService>,
}

impl ComponentService {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        object_store: Arc<ComponentObjectStore>,
        environment_service: Arc<EnvironmentService>,
        deployment_service: Arc<DeploymentService>,
    ) -> Self {
        Self {
            component_repo,
            object_store,
            environment_service,
            deployment_service,
        }
    }

    pub async fn get_staged_component(
        &self,
        component_id: ComponentId,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(component_id = %component_id, "Get staged component");

        let record = self
            .component_repo
            .get_staged_by_id(component_id.0)
            .await?
            .ok_or(ComponentError::ComponentNotFound(component_id))?;

        let environment = self
            .environment_service
            .get(EnvironmentId(record.environment_id), false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(component_id))?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn get_deployed_component(
        &self,
        component_id: ComponentId,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(component_id = %component_id, "Get deployed component");

        // Note: This is in the hot path of worker-service and worker-executor, so it might make sense to offer
        // them a specialized version that bypasses auth / fetching the environment

        let record = self
            .component_repo
            .get_deployed_by_id(component_id.0)
            .await?
            .ok_or(ComponentError::ComponentNotFound(component_id))?;

        let environment = self
            .environment_service
            .get(EnvironmentId(record.environment_id), false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(component_id))?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn get_all_deployed_component_versions(
        &self,
        component_id: ComponentId,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(component_id = %component_id, "Get deployed component");

        let records = self
            .component_repo
            .get_all_deployed_by_id(component_id.0)
            .await?;

        let environment_id: EnvironmentId =
            if let Some(environment_id) = records.first().map(|r| &r.environment_id) {
                (*environment_id).into()
            } else {
                return Err(ComponentError::ComponentNotFound(component_id));
            };

        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(component_id))?;

        let components = records
            .into_iter()
            .map(|r| r.try_into_model(environment.application_id, environment.owner_account_id))
            .collect::<Result<_, _>>()?;

        Ok(components)
    }

    pub async fn get_component_revision(
        &self,
        component_id: ComponentId,
        revision: ComponentRevision,
        include_deleted: bool,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(component_id = %component_id, "Get component revision");

        let record = self
            .component_repo
            .get_by_id_and_revision(component_id.0, revision.into(), include_deleted)
            .await?
            .ok_or(ComponentError::ComponentNotFound(component_id))?;

        let environment = self
            .environment_service
            .get(EnvironmentId(record.environment_id), include_deleted, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ComponentError::ComponentNotFound(component_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentNotFound(component_id))?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn list_staged_components(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    ComponentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        self.list_staged_components_for_environment(&environment, auth)
            .await
    }

    pub async fn list_staged_components_for_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(environment_id = %environment.id, "Get staged components");

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )?;

        let result = self
            .component_repo
            .list_staged(environment.id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into_model(environment.application_id, environment.owner_account_id))
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    pub async fn get_staged_component_by_name(
        &self,
        environment_id: EnvironmentId,
        component_name: &ComponentName,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(
            environment_id = %environment_id,
            component_name = %component_name,
            "Get staged component"
        );

        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    ComponentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentByNameNotFound(component_name.clone()))?;

        let record = self
            .component_repo
            .get_staged_by_name(environment_id.0, &component_name.0)
            .await?
            .ok_or(ComponentError::ComponentByNameNotFound(
                component_name.clone(),
            ))?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn get_deployed_component_by_name(
        &self,
        environment_id: EnvironmentId,
        component_name: &ComponentName,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(
            environment_id = %environment_id,
            component_name = %component_name,
            "Get deployed component"
        );

        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    ComponentError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentByNameNotFound(component_name.clone()))?;

        let record = self
            .component_repo
            .get_deployed_by_name(environment_id.0, &component_name.0)
            .await?
            .ok_or(ComponentError::ComponentByNameNotFound(
                component_name.clone(),
            ))?;

        let converted =
            record.try_into_model(environment.application_id, environment.owner_account_id)?;

        Ok(converted)
    }

    pub async fn list_deployment_components(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(
            environment_id = %environment_id,
            deployment_revision = %deployment_revision.to_string(),
            "Get deployment components"
        );

        let (_, environment) = self
            .deployment_service
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await
            .map_err(|err| match err {
                DeploymentError::ParentEnvironmentNotFound(environment_id) => {
                    ComponentError::ParentEnvironmentNotFound(environment_id)
                }
                DeploymentError::DeploymentNotFound(deployment_revision) => {
                    ComponentError::DeploymentRevisionNotFound(deployment_revision)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )?;

        let result = self
            .component_repo
            .list_by_deployment(environment_id.0, deployment_revision.into())
            .await?
            .into_iter()
            .map(|r| r.try_into_model(environment.application_id, environment.owner_account_id))
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    pub async fn get_deployment_component_by_name(
        &self,
        environment_id: EnvironmentId,
        deployment_revision: DeploymentRevision,
        component_name: &ComponentName,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(
            environment_id = %environment_id,
            deployment_revision = %deployment_revision,
            component_name = %component_name,
            "Get deployed component"
        );

        let (_, environment) = self
            .deployment_service
            .get_deployment_and_environment(environment_id, deployment_revision, auth)
            .await
            .map_err(|err| match err {
                DeploymentError::ParentEnvironmentNotFound(environment_id) => {
                    ComponentError::ParentEnvironmentNotFound(environment_id)
                }
                DeploymentError::DeploymentNotFound(deployment_revision) => {
                    ComponentError::DeploymentRevisionNotFound(deployment_revision)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewComponent,
        )
        .map_err(|_| ComponentError::ComponentByNameNotFound(component_name.clone()))?;

        let record = self
            .component_repo
            .get_by_deployment_and_name(
                environment_id.0,
                deployment_revision.into(),
                &component_name.0,
            )
            .await?
            .ok_or(ComponentError::ComponentByNameNotFound(
                component_name.clone(),
            ))?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn download_component_wasm(
        &self,
        component_id: ComponentId,
        revision: ComponentRevision,
        include_deleted: bool,
        auth: &AuthCtx,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError> {
        let component = self
            .get_component_revision(component_id, revision, include_deleted, auth)
            .await?;

        let stream = self
            .object_store
            .get_stream(
                component.environment_id,
                &component.transformed_object_store_key,
            )
            .await?;

        Ok(stream)
    }
}
