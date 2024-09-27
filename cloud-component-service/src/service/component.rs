use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::project::v1::project_error;
use cloud_common::auth::{CloudAuthCtx, CloudNamespace};
use cloud_common::clients::auth::{AuthServiceError, BaseAuthService};
use cloud_common::clients::limit::{LimitError, LimitService};
use cloud_common::clients::project::{ProjectError, ProjectService};
use cloud_common::model::ProjectAction;
use cloud_common::SafeDisplay;
use golem_common::model::component_metadata::ComponentProcessingError;
use golem_common::model::ProjectId;
use golem_common::model::{ComponentId, ComponentType};
use golem_component_service_base::repo::RepoError;
use golem_component_service_base::service::component::{
    ComponentError as BaseComponentError, ComponentService as BaseComponentService,
};
use golem_service_base::model::*;
use golem_service_base::stream::ByteStream;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Component already exists: {0}")]
    AlreadyExists(ComponentId),
    #[error("Unknown component id: {0}")]
    UnknownComponentId(ComponentId),
    #[error("Unknown versioned component id: {0}")]
    UnknownVersionedComponentId(VersionedComponentId),
    #[error("Unknown project: {0}")]
    UnknownProject(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit exceeded: {0}")]
    LimitExceeded(String),
    #[error(transparent)]
    ComponentProcessing(#[from] ComponentProcessingError),
    #[error(transparent)]
    InternalBaseComponentError(BaseComponentError),
    #[error("Internal repository error: {0}")]
    InternalRepoError(RepoError),
    #[error(transparent)]
    InternalAuthServiceError(AuthServiceError),
    #[error(transparent)]
    InternalLimitError(LimitError),
    #[error(transparent)]
    InternalProjectError(ProjectError),
}

impl SafeDisplay for ComponentError {
    fn to_safe_string(&self) -> String {
        match self {
            ComponentError::AlreadyExists(_) => self.to_string(),
            ComponentError::UnknownComponentId(_) => self.to_string(),
            ComponentError::UnknownVersionedComponentId(_) => self.to_string(),
            ComponentError::UnknownProject(_) => self.to_string(),
            ComponentError::Unauthorized(_) => self.to_string(),
            ComponentError::LimitExceeded(_) => self.to_string(),
            ComponentError::ComponentProcessing(_) => self.to_string(), // TODO: add safe display to ComponentProcessingError
            ComponentError::InternalBaseComponentError(_) => self.to_string(), // TODO: add safe display to BaseComponentError
            ComponentError::InternalRepoError(_) => "Internal repository error".to_string(), // TODO: add safe display to RepoError
            ComponentError::InternalAuthServiceError(inner) => inner.to_safe_string(),
            ComponentError::InternalLimitError(inner) => inner.to_safe_string(),
            ComponentError::InternalProjectError(inner) => inner.to_safe_string(),
        }
    }
}

impl ComponentError {
    pub fn unauthorized<T: Display>(error: T) -> Self {
        ComponentError::Unauthorized(error.to_string())
    }
}

impl From<BaseComponentError> for ComponentError {
    fn from(error: BaseComponentError) -> Self {
        match error {
            BaseComponentError::ComponentProcessingError(v) => {
                ComponentError::ComponentProcessing(v)
            }
            BaseComponentError::AlreadyExists(v) => ComponentError::AlreadyExists(v),
            BaseComponentError::UnknownComponentId(v) => ComponentError::UnknownComponentId(v),
            BaseComponentError::UnknownVersionedComponentId(v) => {
                ComponentError::UnknownVersionedComponentId(v)
            }
            _ => ComponentError::InternalBaseComponentError(error),
        }
    }
}

impl From<RepoError> for ComponentError {
    fn from(error: RepoError) -> Self {
        ComponentError::InternalRepoError(error)
    }
}

impl From<AuthServiceError> for ComponentError {
    fn from(error: AuthServiceError) -> Self {
        match error {
            AuthServiceError::Unauthorized(error) => ComponentError::Unauthorized(error),
            AuthServiceError::Forbidden(error) => ComponentError::Unauthorized(error),
            _ => ComponentError::InternalAuthServiceError(error),
        }
    }
}

impl From<LimitError> for ComponentError {
    fn from(error: LimitError) -> Self {
        match error {
            LimitError::Unauthorized(string) => ComponentError::Unauthorized(string),
            LimitError::LimitExceeded(string) => ComponentError::LimitExceeded(string),
            _ => ComponentError::InternalLimitError(error),
        }
    }
}

impl From<ProjectError> for ComponentError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Server(
                cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError {
                    error: Some(project_error::Error::Unauthorized(e)),
                },
            ) => ComponentError::Unauthorized(e.error),
            ProjectError::Server(
                cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError {
                    error: Some(project_error::Error::LimitExceeded(e)),
                },
            ) => ComponentError::LimitExceeded(e.error),
            ProjectError::Server(
                cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError {
                    error: Some(project_error::Error::NotFound(e)),
                },
            ) => ComponentError::UnknownProject(e.error),
            _ => ComponentError::InternalProjectError(error),
        }
    }
}

#[async_trait]
pub trait ComponentService {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::Component, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        component_type: Option<ComponentType>,
        data: Vec<u8>,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::Component, ComponentError>;

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<u8>, ComponentError>;

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<ByteStream, ComponentError>;

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<Option<Vec<u8>>, ComponentError>;

    async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        component_name: Option<ComponentName>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Component>, ComponentError>;

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Component>, ComponentError>;

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Option<crate::model::Component>, ComponentError>;

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Option<crate::model::Component>, ComponentError>;

    async fn get(
        &self,
        component_id: &ComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Component>, ComponentError>;
}

pub struct ComponentServiceDefault {
    base_component_service: Arc<dyn BaseComponentService<CloudNamespace> + Sync + Send>,
    auth_service: Arc<dyn BaseAuthService + Sync + Send>,
    limit_service: Arc<dyn LimitService + Sync + Send>,
    project_service: Arc<dyn ProjectService + Sync + Send>,
}

impl ComponentServiceDefault {
    pub fn new(
        base_component_service: Arc<dyn BaseComponentService<CloudNamespace> + Sync + Send>,
        auth_service: Arc<dyn BaseAuthService + Sync + Send>,
        limit_service: Arc<dyn LimitService + Sync + Send>,
        project_service: Arc<dyn ProjectService + Sync + Send>,
    ) -> Self {
        ComponentServiceDefault {
            base_component_service,
            auth_service,
            limit_service,
            project_service,
        }
    }

    async fn get_namespace(
        &self,
        project_id: Option<ProjectId>,
        auth: &CloudAuthCtx,
    ) -> Result<CloudNamespace, ComponentError> {
        if let Some(project_id) = project_id.clone() {
            Ok(self
                .is_authorized_by_project(auth, &project_id, &ProjectAction::ViewComponent)
                .await?)
        } else {
            let project = self.project_service.get_default(&auth.token_secret).await?;
            Ok(CloudNamespace::from(project))
        }
    }
}

#[async_trait]
impl ComponentService for ComponentServiceDefault {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        component_name: &ComponentName,
        component_type: ComponentType,
        data: Vec<u8>,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::Component, ComponentError> {
        let component_id = ComponentId::new_v4();

        let namespace = self.get_namespace(project_id, auth).await?;

        self.base_component_service
            .find_id_by_name(component_name, &namespace)
            .await?
            .map_or(Ok(()), |id| Err(ComponentError::AlreadyExists(id)))?;

        let component_size: u64 = data.len() as u64;

        self.limit_service
            .update_component_limit(
                &namespace.account_id,
                &component_id,
                1,
                component_size as i64,
            )
            .await?;

        let component = self
            .base_component_service
            .create(
                &component_id,
                component_name,
                component_type,
                data.clone(),
                &namespace,
            )
            .await?;

        Ok(component.into())
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        component_type: Option<ComponentType>,
        data: Vec<u8>,
        auth: &CloudAuthCtx,
    ) -> Result<crate::model::Component, ComponentError> {
        let namespace = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;

        let component_size: u64 = data.len() as u64;

        self.limit_service
            .update_component_limit(
                &namespace.account_id,
                component_id,
                0,
                component_size as i64,
            )
            .await?;

        let component = self
            .base_component_service
            .update(component_id, data.clone(), component_type, &namespace)
            .await?;

        Ok(component.into())
    }

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<u8>, ComponentError> {
        let namespace = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;

        let data = self
            .base_component_service
            .download(component_id, version, &namespace)
            .await?;

        Ok(data)
    }

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<ByteStream, ComponentError> {
        let namespace = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;

        let stream = self
            .base_component_service
            .download_stream(component_id, version, &namespace)
            .await?;
        Ok(stream)
    }

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &CloudAuthCtx,
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        let namespace = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;

        let result = self
            .base_component_service
            .get_protected_data(component_id, version, &namespace)
            .await?;

        Ok(result)
    }

    async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        component_name: Option<ComponentName>,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        let namespace = self.get_namespace(project_id, auth).await?;

        let result = self
            .base_component_service
            .find_by_name(component_name, &namespace)
            .await?;

        Ok(result.into_iter().map(|c| c.into()).collect())
    }

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        let namespace = self
            .is_authorized_by_project(auth, project_id, &ProjectAction::ViewComponent)
            .await?;

        let result = self
            .base_component_service
            .find_by_name(None, &namespace)
            .await?;
        Ok(result.into_iter().map(|c| c.into()).collect())
    }

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Option<crate::model::Component>, ComponentError> {
        let namespace = self
            .is_authorized_by_component(
                auth,
                &component_id.component_id,
                &ProjectAction::ViewComponent,
            )
            .await?;

        let result = self
            .base_component_service
            .get_by_version(component_id, &namespace)
            .await?;

        Ok(result.map(|c| c.into()))
    }

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Option<crate::model::Component>, ComponentError> {
        let namespace = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let result = self
            .base_component_service
            .get_latest_version(component_id, &namespace)
            .await?;
        Ok(result.map(|c| c.into()))
    }

    async fn get(
        &self,
        component_id: &ComponentId,
        auth: &CloudAuthCtx,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        let namespace = self
            .is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let result = self
            .base_component_service
            .get(component_id, &namespace)
            .await?;

        Ok(result.into_iter().map(|c| c.into()).collect())
    }
}

impl ComponentServiceDefault {
    async fn is_authorized_by_component(
        &self,
        auth: &CloudAuthCtx,
        component_id: &ComponentId,
        action: &ProjectAction,
    ) -> Result<CloudNamespace, ComponentError> {
        let namespace = self
            .base_component_service
            .get_namespace(component_id)
            .await?;

        match namespace {
            Some(namespace) => {
                self.is_authorized_by_project(auth, &namespace.project_id, action)
                    .await
            }
            None => Err(ComponentError::Unauthorized(format!(
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
    ) -> Result<CloudNamespace, ComponentError> {
        let namespace = self
            .auth_service
            .is_authorized(project_id, action.clone(), auth)
            .await?;
        Ok(namespace)
    }
}
