use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::ProjectAction;
use golem_common::model::ComponentId;
use golem_common::model::ProjectId;
use golem_component_service_base::service::component_compilation::ComponentCompilationService;
use golem_component_service_base::service::component_processor::process_component;
use golem_component_service_base::service::component_processor::ComponentProcessingError;
use tap::TapFallible;
use tracing::{error, info};

use super::plan_limit::CheckLimitResult;
use crate::auth::AccountAuthorisation;
use crate::model::*;
use crate::repo::account_uploads::AccountUploadsRepo;
use crate::repo::component::ComponentRepo;
use crate::repo::RepoError;
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};
use crate::service::project::{ProjectError, ProjectService};
use crate::service::project_auth::{ProjectAuthorisationError, ProjectAuthorisationService};
use golem_service_base::model::*;
use golem_service_base::service::component_object_store::ComponentObjectStore;
use golem_service_base::stream::ByteStream;

#[derive(Debug, thiserror::Error)]
pub enum ComponentError {
    #[error("Component already exists: {0}")]
    AlreadyExists(ComponentId),
    #[error("Unknown component id: {0}")]
    UnknownComponentId(ComponentId),
    #[error("Unknown versioned component id: {0}")]
    UnknownVersionedComponentId(VersionedComponentId),
    #[error("Unknown project id: {0}")]
    UnknownProjectId(ProjectId),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit exceeded: {0}")]
    LimitExceeded(String),
    #[error(transparent)]
    ComponentProcessing(#[from] ComponentProcessingError),
    #[error("Internal error: {0}")]
    Internal(anyhow::Error),
}

impl ComponentError {
    fn internal<E, C>(error: E, context: C) -> Self
    where
        E: Display + std::fmt::Debug + Send + Sync + 'static,
        C: Display + Send + Sync + 'static,
    {
        ComponentError::Internal(anyhow::Error::msg(error).context(context))
    }

    pub fn unauthorized<T: Display>(error: T) -> Self {
        ComponentError::Unauthorized(error.to_string())
    }
}

impl From<RepoError> for ComponentError {
    fn from(error: RepoError) -> Self {
        let RepoError::Internal(error) = error;
        ComponentError::Internal(anyhow::Error::msg(error))
    }
}

impl From<PlanLimitError> for ComponentError {
    fn from(error: PlanLimitError) -> Self {
        match error {
            PlanLimitError::Unauthorized(error) => ComponentError::Unauthorized(error),
            PlanLimitError::Internal(error) => {
                ComponentError::Internal(anyhow::Error::msg(error.to_string()))
            }
            // TODO: This looks wrong.
            PlanLimitError::AccountIdNotFound(_) => {
                ComponentError::Internal(anyhow::Error::msg("Account not found"))
            }
            PlanLimitError::ProjectIdNotFound(project_id) => {
                ComponentError::UnknownProjectId(project_id)
            }
            PlanLimitError::ComponentIdNotFound(component_id) => {
                ComponentError::UnknownComponentId(component_id)
            }
            PlanLimitError::LimitExceeded(error) => ComponentError::LimitExceeded(error),
        }
    }
}

impl From<ProjectError> for ComponentError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Unauthorized(error) => ComponentError::Unauthorized(error),
            ProjectError::LimitExceeded(error) => ComponentError::LimitExceeded(error),
            ProjectError::Internal(error) => ComponentError::Internal(anyhow::Error::msg(error)),
        }
    }
}

impl From<ProjectAuthorisationError> for ComponentError {
    fn from(error: ProjectAuthorisationError) -> Self {
        match error {
            ProjectAuthorisationError::Unauthorized(error) => ComponentError::Unauthorized(error),
            ProjectAuthorisationError::Internal(error) => {
                ComponentError::Internal(anyhow::Error::msg(error))
            }
        }
    }
}

#[async_trait]
pub trait ComponentService {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        component_name: &ComponentName,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Component, ComponentError>;

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Component, ComponentError>;

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<u8>, ComponentError>;

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AccountAuthorisation,
    ) -> Result<ByteStream, ComponentError>;

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AccountAuthorisation,
    ) -> Result<Option<Vec<u8>>, ComponentError>;

    async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        component_name: Option<ComponentName>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError>;

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError>;

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Component>, ComponentError>;

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Component>, ComponentError>;

    async fn get(
        &self,
        component_id: &ComponentId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError>;
}

pub struct ComponentServiceDefault {
    account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
    plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
    object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
    project_service: Arc<dyn ProjectService + Sync + Send>,
    project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
    component_compilation_service: Arc<dyn ComponentCompilationService + Sync + Send>,
}

impl ComponentServiceDefault {
    pub fn new(
        account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
        component_repo: Arc<dyn ComponentRepo + Sync + Send>,
        plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
        object_store: Arc<dyn ComponentObjectStore + Sync + Send>,
        project_service: Arc<dyn ProjectService + Sync + Send>,
        project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
        component_compilation_service: Arc<dyn ComponentCompilationService + Sync + Send>,
    ) -> Self {
        ComponentServiceDefault {
            account_uploads_repo,
            component_repo,
            plan_limit_service,
            object_store,
            project_service,
            project_auth_service,
            component_compilation_service,
        }
    }
}

#[async_trait]
impl ComponentService for ComponentServiceDefault {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        component_name: &ComponentName,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Component, ComponentError> {
        let pid = project_id
            .clone()
            .map_or("default".to_string(), |n| n.0.to_string());
        let tn = component_name.0.clone();
        info!("Creating component for project {} with name {}", pid, tn);

        let project_id = if let Some(project_id) = project_id.clone() {
            project_id
        } else {
            let project = self.project_service.get_own_default(auth).await?;
            project.project_id
        };

        self.is_authorized_by_project(auth, &project_id, &ProjectAction::CreateComponent)
            .await?;
        self.check_plan_limits(&project_id).await?;

        self.check_new_name(&project_id, component_name).await?;

        let plan_limit = self
            .plan_limit_service
            .get_project_limits(&project_id)
            .await?;

        let account_id = plan_limit.account_id;

        let storage_limit = self
            .plan_limit_service
            .check_storage_limit(&account_id)
            .await?;

        let upload_limit = self
            .plan_limit_service
            .check_upload_limit(&account_id)
            .await?;

        info!("create_component limits verified");

        self.validate_limits(storage_limit, upload_limit, &data)
            .await?;

        let metadata = process_component(&data)?;

        let component_id = ComponentId::new_v4();

        info!("Component {component_id} metadata: {metadata:?}");

        let versioned_component_id = VersionedComponentId {
            component_id,
            version: 0,
        };

        let user_component_id = UserComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };
        let protected_component_id = ProtectedComponentId {
            versioned_component_id: versioned_component_id.clone(),
        };

        info!("Pushing {:?}", user_component_id);

        let component_size: u64 = data
            .len()
            .try_into()
            .map_err(|e| ComponentError::internal(e, "Failed to convert data length"))?;

        tokio::try_join!(
            self.upload_user_component(&user_component_id, data.clone()),
            self.upload_protected_component(&protected_component_id, data)
        )?;

        info!("ComponentService create_component object store finished");

        let component = crate::model::Component {
            component_name: component_name.clone(),
            component_size,
            project_id: project_id.clone(),
            metadata,
            versioned_component_id,
            user_component_id,
            protected_component_id,
        };

        self.component_repo
            .upsert(&component.clone().into())
            .await?;

        info!("ComponentService create_component finished successfully");

        self.component_compilation_service
            .enqueue_compilation(&component.versioned_component_id.component_id, 0)
            .await;

        Ok(component)
    }

    async fn update(
        &self,
        component_id: &ComponentId,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Component, ComponentError> {
        info!("Updating component {}", component_id.0);
        self.is_authorized_by_component(auth, component_id, &ProjectAction::UpdateComponent)
            .await?;

        let plan_limit = self
            .plan_limit_service
            .get_component_limits(component_id)
            .await?;

        let account_id = plan_limit.account_id;

        let storage_limit = self
            .plan_limit_service
            .check_storage_limit(&account_id)
            .await?;

        let upload_limit = self
            .plan_limit_service
            .check_upload_limit(&account_id)
            .await?;

        info!("update_component limits verified");

        self.validate_limits(storage_limit, upload_limit, &data)
            .await?;

        let metadata = process_component(&data)?;

        let next_component = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?
            .map(crate::model::Component::try_from)
            .transpose()
            .map_err(|e| ComponentError::internal(e, "Failed to convert Component"))?
            .map(crate::model::Component::next_version)
            .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?;

        info!("Pushing {:?}", next_component.user_component_id);

        let component_size: u64 = data
            .len()
            .try_into()
            .map_err(|e| ComponentError::internal(e, "Failed to convert data length"))?;

        tokio::try_join!(
            self.upload_user_component(&next_component.user_component_id, data.clone()),
            self.upload_protected_component(&next_component.protected_component_id, data)
        )?;

        info!("ComponentService update_component object store finished");

        let component = crate::model::Component {
            component_size,
            metadata,
            ..next_component
        };

        self.component_compilation_service
            .enqueue_compilation(
                &component.versioned_component_id.component_id,
                component.versioned_component_id.version,
            )
            .await;

        self.component_repo
            .upsert(&component.clone().into())
            .await?;

        Ok(component)
    }

    async fn download(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<u8>, ComponentError> {
        self.is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let versioned_component_id = {
            match version {
                Some(version) => VersionedComponentId {
                    component_id: component_id.clone(),
                    version,
                },
                None => self
                    .component_repo
                    .get_latest_version(&component_id.0)
                    .await?
                    .map(crate::model::Component::try_from)
                    .transpose()
                    .map_err(|e| ComponentError::internal(e, "Failed to convert component"))?
                    .map(|t| t.versioned_component_id)
                    .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?,
            }
        };
        info!(
            "Downloading component {} version {}",
            component_id, versioned_component_id.version
        );

        let id = ProtectedComponentId {
            versioned_component_id,
        };

        self.object_store
            .get(&self.get_protected_object_store_key(&id))
            .await
            .tap_err(|e| error!("Error downloading component: {}", e))
            .map_err(|e| ComponentError::internal(e.to_string(), "Error downloading component"))
    }

    async fn download_stream(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AccountAuthorisation,
    ) -> Result<ByteStream, ComponentError> {
        self.is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let versioned_component_id = {
            match version {
                Some(version) => VersionedComponentId {
                    component_id: component_id.clone(),
                    version,
                },
                None => self
                    .component_repo
                    .get_latest_version(&component_id.0)
                    .await?
                    .map(crate::model::Component::try_from)
                    .transpose()
                    .map_err(|e| ComponentError::internal(e, "Failed to convert component"))?
                    .map(|t| t.versioned_component_id)
                    .ok_or(ComponentError::UnknownComponentId(component_id.clone()))?,
            }
        };
        info!(
            "Downloading component {} version {}",
            component_id, versioned_component_id.version
        );

        let id = ProtectedComponentId {
            versioned_component_id,
        };

        let stream = self
            .object_store
            .get_stream(&self.get_protected_object_store_key(&id))
            .await;

        Ok(stream)
    }

    async fn get_protected_data(
        &self,
        component_id: &ComponentId,
        version: Option<u64>,
        auth: &AccountAuthorisation,
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        info!(
            "Getting component  {} version {} data",
            component_id,
            version.map_or("N/A".to_string(), |v| v.to_string())
        );

        self.is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;

        let latest_component = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?;

        let v = match latest_component {
            Some(component) => match version {
                Some(v) if v <= component.version as u64 => v,
                None => component.version as u64,
                _ => {
                    return Ok(None);
                }
            },
            None => {
                return Ok(None);
            }
        };

        let versioned_component_id = VersionedComponentId {
            component_id: component_id.clone(),
            version: v,
        };

        let protected_id = ProtectedComponentId {
            versioned_component_id,
        };

        let object_key = self.get_protected_object_store_key(&protected_id);

        let result = self
            .object_store
            .get(&object_key)
            .await
            .tap_err(|e| error!("Error retrieving protected component: {}", e))
            .map_err(|e| {
                ComponentError::internal(e.to_string(), "Error retrieving protected component")
            })?;

        Ok(Some(result))
    }

    async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        component_name: Option<ComponentName>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        let pid = project_id
            .clone()
            .map_or("default".to_string(), |n| n.0.to_string());
        let tn = component_name.clone().map_or("N/A".to_string(), |n| n.0);
        info!("Getting component by project {} and name {}", pid, tn);

        let project_id = if let Some(project_id) = project_id.clone() {
            project_id
        } else {
            let project = self.project_service.get_own_default(auth).await?;
            project.project_id
        };

        self.is_authorized_by_project(auth, &project_id, &ProjectAction::ViewComponent)
            .await?;

        let result = match component_name {
            Some(name) => {
                self.component_repo
                    .get_by_project_and_name(&project_id.0, &name.0)
                    .await?
            }
            None => self.component_repo.get_by_project(&project_id.0).await?,
        };

        result
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<_, String>>()
            .map_err(|e| ComponentError::internal(e, "Failed to convert component"))
    }

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        info!("Getting component by project {}", project_id);
        self.is_authorized_by_project(auth, project_id, &ProjectAction::ViewComponent)
            .await?;

        let result = self.component_repo.get_by_project(&project_id.0).await?;

        result
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<_, String>>()
            .map_err(|e| ComponentError::internal(e, "Failed to convert component"))
    }

    async fn get(
        &self,
        component_id: &ComponentId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        info!("Getting component {}", component_id);
        self.is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let result = self.component_repo.get(&component_id.0).await?;

        result
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<_, String>>()
            .map_err(|e| ComponentError::internal(e, "Failed to convert component"))
    }

    async fn get_by_version(
        &self,
        component_id: &VersionedComponentId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Component>, ComponentError> {
        info!(
            "Getting component {} version {}",
            component_id.component_id, component_id.version
        );
        self.is_authorized_by_component(
            auth,
            &component_id.component_id,
            &ProjectAction::ViewComponent,
        )
        .await?;
        let result = self
            .component_repo
            .get_by_version(&component_id.component_id.0, component_id.version)
            .await?;

        result
            .map(crate::model::Component::try_from)
            .transpose()
            .map_err(|e| ComponentError::internal(e, "Failed to convert component"))
    }

    async fn get_latest_version(
        &self,
        component_id: &ComponentId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Component>, ComponentError> {
        info!("Getting component {} latest version", component_id);
        self.is_authorized_by_component(auth, component_id, &ProjectAction::ViewComponent)
            .await?;
        let result = self
            .component_repo
            .get_latest_version(&component_id.0)
            .await?;

        result
            .map(crate::model::Component::try_from)
            .transpose()
            .map_err(|e| ComponentError::internal(e, "Failed to convert component"))
    }
}

impl ComponentServiceDefault {
    async fn is_authorized_by_component(
        &self,
        auth: &AccountAuthorisation,
        component_id: &ComponentId,
        required_action: &ProjectAction,
    ) -> Result<(), ComponentError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            let permissions = self
                .project_auth_service
                .get_by_component(component_id, auth)
                .await
                .tap_err(|e| error!("Error getting component permissions: {:?}", e))?;

            if permissions.actions.actions.contains(required_action) {
                Ok(())
            } else {
                Err(ComponentError::Unauthorized(format!(
                    "Account unauthorized to perform action on component {}: {}",
                    component_id.0, required_action
                )))
            }
        }
    }

    async fn is_authorized_by_project(
        &self,
        auth: &AccountAuthorisation,
        project_id: &ProjectId,
        required_action: &ProjectAction,
    ) -> Result<(), ComponentError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            let permissions = self
                .project_auth_service
                .get_by_project(project_id, auth)
                .await
                .tap_err(|e| error!("Error getting component permissions: {:?}", e))?;

            if permissions.actions.actions.contains(required_action) {
                Ok(())
            } else {
                Err(ComponentError::Unauthorized(format!(
                    "Account unauthorized to perform action on project {}: {}",
                    project_id.0, required_action
                )))
            }
        }
    }

    async fn check_plan_limits(&self, project_id: &ProjectId) -> Result<(), ComponentError> {
        let limits = self
            .plan_limit_service
            .check_component_limit(project_id)
            .await
            .tap_err(|e| error!("Error checking limit for project {}: {:?}", project_id, e))?;

        if limits.not_in_limit() {
            Err(ComponentError::LimitExceeded(format!(
                "Component limit exceeded for project: {} (limit: {})",
                project_id.0, limits.limit
            )))
        } else {
            Ok(())
        }
    }

    async fn check_new_name(
        &self,
        project_id: &ProjectId,
        component_name: &ComponentName,
    ) -> Result<(), ComponentError> {
        let existing_components = self
            .component_repo
            .get_by_project_and_name(&project_id.0, &component_name.0)
            .await
            .tap_err(|e| error!("Error getting existing components: {}", e))?;

        existing_components
            .into_iter()
            .next()
            .map(crate::model::Component::try_from)
            .transpose()
            .map_err(|e| ComponentError::internal(e, "Failed to convert component"))?
            .map_or(Ok(()), |t| {
                Err(ComponentError::AlreadyExists(
                    t.versioned_component_id.component_id,
                ))
            })
    }

    fn get_user_object_store_key(&self, id: &UserComponentId) -> String {
        id.slug()
    }

    fn get_protected_object_store_key(&self, id: &ProtectedComponentId) -> String {
        id.slug()
    }

    async fn upload_user_component(
        &self,
        user_component_id: &UserComponentId,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        info!("Uploading user component: {:?}", user_component_id);

        self.object_store
            .put(&self.get_user_object_store_key(user_component_id), data)
            .await
            .tap_err(|e| error!("Error uploading user component: {}", e))
            .map_err(|e| ComponentError::internal(e.to_string(), "Failed to upload user component"))
    }

    async fn upload_protected_component(
        &self,
        protected_component_id: &ProtectedComponentId,
        data: Vec<u8>,
    ) -> Result<(), ComponentError> {
        info!(
            "Uploading protected component: {:?}",
            protected_component_id
        );

        self.object_store
            .put(
                &self.get_protected_object_store_key(protected_component_id),
                data,
            )
            .await
            .tap_err(|e| error!("Error uploading protected component: {}", e))
            .map_err(|e| {
                ComponentError::internal(e.to_string(), "Failed to upload protected component")
            })
    }

    async fn validate_limits(
        &self,
        storage_limit: CheckLimitResult,
        upload_limit: CheckLimitResult,
        data: &[u8],
    ) -> Result<(), ComponentError> {
        let num_bytes: i32 = data
            .len()
            .try_into()
            .map_err(|e| ComponentError::internal(e, "Failed to convert data length"))?;

        if num_bytes > 50000000 {
            Err(ComponentError::LimitExceeded(
                "Component size limit exceeded (limit: 50MB)".into(),
            ))
        } else if !storage_limit.add(num_bytes.into()).in_limit() {
            Err(ComponentError::LimitExceeded(format!(
                "Storage limit exceeded for account: {} (limit: {} MB)",
                storage_limit.account_id.value,
                storage_limit.limit / 1000000
            )))
        } else if !upload_limit.add(num_bytes.into()).in_limit() {
            Err(ComponentError::LimitExceeded(format!(
                "Upload limit exceeded for account: {} (limit: {} MB)",
                upload_limit.account_id.value,
                upload_limit.limit / 1000000
            )))
        } else {
            self.account_uploads_repo
                .update(&upload_limit.account_id, num_bytes)
                .await?;

            Ok(())
        }
    }
}

#[derive(Default)]
pub struct ComponentServiceNoOp {}

#[async_trait]
impl ComponentService for ComponentServiceNoOp {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        _component_name: &ComponentName,
        _data: Vec<u8>,
        _auth: &AccountAuthorisation,
    ) -> Result<crate::model::Component, ComponentError> {
        let fake_component = crate::model::Component {
            component_name: ComponentName("fake".to_string()),
            component_size: 0,
            project_id: project_id.unwrap_or(ProjectId::new_v4()),
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
            },
            versioned_component_id: VersionedComponentId {
                component_id: ComponentId::new_v4(),
                version: 0,
            },
            user_component_id: UserComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
            },
        };

        Ok(fake_component)
    }

    async fn update(
        &self,
        _component_id: &ComponentId,
        _data: Vec<u8>,
        _auth: &AccountAuthorisation,
    ) -> Result<crate::model::Component, ComponentError> {
        let fake_component = crate::model::Component {
            component_name: ComponentName("fake".to_string()),
            component_size: 0,
            project_id: ProjectId::new_v4(),
            metadata: ComponentMetadata {
                exports: vec![],
                producers: vec![],
            },
            versioned_component_id: VersionedComponentId {
                component_id: ComponentId::new_v4(),
                version: 0,
            },
            user_component_id: UserComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
            },
            protected_component_id: ProtectedComponentId {
                versioned_component_id: VersionedComponentId {
                    component_id: ComponentId::new_v4(),
                    version: 0,
                },
            },
        };

        Ok(fake_component)
    }

    async fn download(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<u8>, ComponentError> {
        Ok(vec![])
    }

    async fn download_stream(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
        _auth: &AccountAuthorisation,
    ) -> Result<ByteStream, ComponentError> {
        Ok(ByteStream::empty())
    }

    async fn get_protected_data(
        &self,
        _component_id: &ComponentId,
        _version: Option<u64>,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<Vec<u8>>, ComponentError> {
        Ok(None)
    }

    async fn find_by_project_and_name(
        &self,
        _project_id: Option<ProjectId>,
        _component_name: Option<ComponentName>,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        Ok(vec![])
    }

    async fn get_by_project(
        &self,
        _project_id: &ProjectId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        Ok(vec![])
    }

    async fn get_by_version(
        &self,
        _component_id: &VersionedComponentId,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Component>, ComponentError> {
        Ok(None)
    }

    async fn get_latest_version(
        &self,
        _component_id: &ComponentId,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Component>, ComponentError> {
        Ok(None)
    }

    async fn get(
        &self,
        _component_id: &ComponentId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Component>, ComponentError> {
        Ok(vec![])
    }
}
