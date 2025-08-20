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

use std::sync::Arc;
use crate::error::ComponentError;
use crate::service::component::ComponentService;
use golem_common::base_model::{ComponentId, ProjectId};
use golem_common::model::auth::{AuthCtx, ProjectAction};
use golem_common::model::component::ComponentOwner;
use golem_service_base::clients::auth::AuthService;
use golem_service_base::clients::project::ProjectService;

pub mod agent_types;
pub mod component;
pub mod plugin;

async fn is_authorized_by_project(
    auth_service: &AuthService,
    auth: &AuthCtx,
    project_id: &ProjectId,
    action: &ProjectAction,
) -> Result<ComponentOwner, ComponentError> {
    let namespace = auth_service
        .authorize_project_action(project_id, action.clone(), auth)
        .await?;

    Ok(ComponentOwner {
        account_id: namespace.account_id,
        project_id: namespace.project_id,
    })
}

async fn is_authorized_by_project_or_default(
    auth_service: &AuthService,
    project_service: &Arc<dyn ProjectService>,
    auth: &AuthCtx,
    project_id: Option<ProjectId>,
    action: &ProjectAction,
) -> Result<ComponentOwner, ComponentError> {
    if let Some(project_id) = project_id.clone() {
        is_authorized_by_project(auth_service, auth, &project_id, action).await
    } else {
        let project = project_service.get_default(&auth.token_secret).await?;
        Ok(ComponentOwner {
            account_id: project.owner_account_id,
            project_id: project.id,
        })
    }
}

async fn is_authorized_by_component(
    auth_service: &AuthService,
    component_service: &Arc<dyn ComponentService>,
    auth: &AuthCtx,
    component_id: &ComponentId,
    action: &ProjectAction,
) -> Result<ComponentOwner, ComponentError> {
    let owner = component_service.get_owner(component_id).await?;

    match owner {
        Some(owner) => {
            is_authorized_by_project(auth_service, auth, &owner.project_id, action).await
        }
        None => Err(ComponentError::Unauthorized(format!(
            "Account unauthorized to perform action on component {}: {}",
            component_id.0, action
        ))),
    }
}
