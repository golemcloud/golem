use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_service_base::model::Component as BaseComponent;
use golem_service_base::model::VersionedComponentId;
use golem_worker_service_base::service::component::{
    ComponentResult, ComponentService as BaseComponentService,
    ComponentServiceError as BaseComponentError,
};

use crate::auth::AccountAuthorisation;
use crate::service::component::{ComponentError, ComponentService};

#[derive(Clone)]
pub struct ComponentServiceWrapper {
    component_service: Arc<dyn ComponentService + Send + Sync>,
}

impl ComponentServiceWrapper {
    pub fn new(component_service: Arc<dyn ComponentService + Send + Sync>) -> Self {
        Self { component_service }
    }
}

#[async_trait]
impl BaseComponentService<AccountAuthorisation> for ComponentServiceWrapper {
    async fn get_by_version(
        &self,
        component_id: &ComponentId,
        version: u64,
        auth_ctx: &AccountAuthorisation,
    ) -> ComponentResult<BaseComponent> {
        let versioned = VersionedComponentId {
            component_id: component_id.clone(),
            version,
        };

        let component = self
            .component_service
            .get_by_version(&versioned, auth_ctx)
            .await?
            .ok_or(BaseComponentError::NotFound(format!(
                "Component not found: {component_id}",
            )))?;

        let component = convert_component(component);

        Ok(component)
    }

    async fn get_latest(
        &self,
        component_id: &ComponentId,
        auth_ctx: &AccountAuthorisation,
    ) -> ComponentResult<BaseComponent> {
        let component = self
            .component_service
            .get_latest_version(component_id, auth_ctx)
            .await?
            .ok_or(BaseComponentError::NotFound(format!(
                "Component not found: {component_id}",
            )))?;

        let component = convert_component(component);

        Ok(component)
    }
}

impl From<ComponentError> for BaseComponentError {
    fn from(error: ComponentError) -> Self {
        match error {
            ComponentError::AlreadyExists(_) => {
                BaseComponentError::AlreadyExists(error.to_string())
            }
            ComponentError::UnknownComponentId(_)
            | ComponentError::UnknownVersionedComponentId(_)
            | ComponentError::UnknownProjectId(_) => {
                BaseComponentError::NotFound(error.to_string())
            }
            ComponentError::Unauthorized(e) => BaseComponentError::Unauthorized(e),
            ComponentError::LimitExceeded(e) => BaseComponentError::Forbidden(e),
            ComponentError::ComponentProcessing(e) => {
                BaseComponentError::BadRequest(vec![e.to_string()])
            }
            ComponentError::Internal(e) => BaseComponentError::Internal(anyhow::Error::msg(e)),
        }
    }
}

fn convert_component(component: crate::model::Component) -> BaseComponent {
    BaseComponent {
        versioned_component_id: component.versioned_component_id,
        user_component_id: component.user_component_id,
        protected_component_id: component.protected_component_id,
        component_name: component.component_name,
        component_size: component.component_size,
        metadata: component.metadata,
    }
}
