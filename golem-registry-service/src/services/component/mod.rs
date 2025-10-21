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
use super::component_transformer_plugin_caller::{self};
use super::environment::EnvironmentService;
use crate::model::auth::AuthCtx;
use crate::model::component::Component;
use crate::repo::component::ComponentRepo;
use crate::services::environment::EnvironmentError;
use futures::stream::BoxStream;
use golem_common::model::auth::EnvironmentAction;
use golem_common::model::component::ComponentId;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use std::sync::Arc;
use tracing::info;

pub struct ComponentService {
    component_repo: Arc<dyn ComponentRepo>,
    object_store: Arc<ComponentObjectStore>,
    environment_service: Arc<EnvironmentService>,
}

impl ComponentService {
    pub fn new(
        component_repo: Arc<dyn ComponentRepo>,
        object_store: Arc<ComponentObjectStore>,
        environment_service: Arc<EnvironmentService>,
    ) -> Self {
        Self {
            component_repo,
            object_store,
            environment_service,
        }
    }

    pub async fn get_component(
        &self,
        component_id: &ComponentId,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(component_id = %component_id, "Get component");

        let record = self
            .component_repo
            .get_staged_by_id(&component_id.0)
            .await?
            .ok_or(ComponentError::NotFound)?;

        let environment = self.environment_service
            .get_and_authorize(
                &EnvironmentId(record.environment_id).clone(),
                EnvironmentAction::ViewComponent,
                auth,
            )
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_)
                | EnvironmentError::Unauthorized(_) => ComponentError::NotFound,
                other => other.into(),
            })?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn get_component_revision(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(component_id = %component_id, "Get component revision");

        let record = self
            .component_repo
            .get_by_id_and_revision(&component_id.0, revision.0 as i64)
            .await?
            .ok_or(ComponentError::NotFound)?;

        let environment = self.environment_service
            .get_and_authorize(
                &EnvironmentId(record.environment_id),
                EnvironmentAction::ViewComponent,
                auth,
            )
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_)
                | EnvironmentError::Unauthorized(_) => ComponentError::NotFound,
                other => other.into(),
            })?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn list_staged_components(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(environment_id = %environment_id, "Get staged components");

        let environment = self.environment_service
            .get_and_authorize(environment_id, EnvironmentAction::ViewComponent, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) | EnvironmentError::Unauthorized(_) => {
                    ComponentError::NotFound
                }
                other => other.into(),
            })?;

        let result = self
            .component_repo
            .list_staged(&environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into_model(environment.application_id.clone(), environment.owner_account_id.clone()))
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    pub async fn get_staged_component(
        &self,
        environment_id: EnvironmentId,
        component_name: ComponentName,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(
            environment_id = %environment_id,
            component_name = %component_name,
            "Get staged component"
        );

        let record = self
            .component_repo
            .get_staged_by_name(&environment_id.0, &component_name.0)
            .await?
            .ok_or(ComponentError::NotFound)?;

        let environment = self.environment_service
            .get_and_authorize(
                &EnvironmentId(record.environment_id),
                EnvironmentAction::ViewComponent,
                auth,
            )
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_)
                | EnvironmentError::Unauthorized(_) => ComponentError::NotFound,
                other => other.into(),
            })?;

        Ok(record.try_into_model(environment.application_id, environment.owner_account_id)?)
    }

    pub async fn list_deployed_components(
        &self,
        environment_id: &EnvironmentId,
        deployment_revision_id: DeploymentRevision,
        auth: &AuthCtx,
    ) -> Result<Vec<Component>, ComponentError> {
        info!(
            environment_id = %environment_id,
            deployment_revision_id = %deployment_revision_id.0.to_string(),
            "Get deployed components"
        );

        let environment = self.environment_service
            .get_and_authorize(environment_id, EnvironmentAction::ViewComponent, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    ComponentError::ParentEnvironmentNotFound(environment_id)
                }
                EnvironmentError::Unauthorized(inner) => ComponentError::Unauthorized(inner),
                other => other.into(),
            })?;

        let result = self
            .component_repo
            .list_by_deployment(&environment_id.0, deployment_revision_id.0 as i64)
            .await?
            .into_iter()
            .map(|r| r.try_into_model(environment.application_id.clone(), environment.owner_account_id.clone()))
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    pub async fn get_deployed_component(
        &self,
        environment_id: EnvironmentId,
        deployment_revision_id: DeploymentRevision,
        component_name: ComponentName,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentError> {
        info!(
            environment_id = %environment_id,
            deployment_revision_id = %deployment_revision_id,
            component_name = %component_name,
            "Get deployed component"
        );

        let record = self
            .component_repo
            .get_by_deployment_and_name(
                &environment_id.0,
                deployment_revision_id.0 as i64,
                &component_name.0,
            )
            .await?
            .ok_or(ComponentError::NotFound)?;

        let environment = self.environment_service
            .get_and_authorize(
                &EnvironmentId(record.environment_id),
                EnvironmentAction::ViewComponent,
                auth,
            )
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_)
                | EnvironmentError::Unauthorized(_) => ComponentError::NotFound,
                other => other.into(),
            })?;

        Ok(record.try_into_model(environment.application_id,  environment.owner_account_id)?)
    }

    pub async fn download_component_wasm(
        &self,
        component_id: &ComponentId,
        revision: ComponentRevision,
        auth: &AuthCtx,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, anyhow::Error>>, ComponentError> {
        let component = self
            .get_component_revision(component_id, revision, auth)
            .await?;

        let stream = self
            .object_store
            .get_stream(
                &component.environment_id,
                &component.transformed_object_store_key,
            )
            .await?;

        Ok(stream)
    }
}
