use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::ProjectId;
use golem_common::model::TemplateId;
use golem_wasm_ast::analysis::{AnalysisContext, AnalysisFailure};
use golem_wasm_ast::component::Component;
use golem_wasm_ast::IgnoreAllButMetadata;
use tap::TapFallible;
use tracing::{error, info};

use super::plan_limit::CheckLimitResult;
use crate::auth::AccountAuthorisation;
use crate::model::*;
use crate::repo::account_uploads::AccountUploadsRepo;
use crate::repo::template::TemplateRepo;
use crate::repo::RepoError;
use crate::service::plan_limit::{PlanLimitError, PlanLimitService};
use crate::service::project::{ProjectError, ProjectService};
use crate::service::project_auth::{ProjectAuthorisationError, ProjectAuthorisationService};
use golem_service_base::model::*;
use golem_service_base::service::template_object_store::TemplateObjectStore;

#[derive(Debug, Clone)]
pub enum TemplateError {
    AlreadyExists(TemplateId),
    UnknownTemplateId(TemplateId),
    UnknownVersionedTemplateId(VersionedTemplateId),
    UnknownProjectId(ProjectId),
    Internal(String),
    IOError(String),
    Unauthorized(String),
    LimitExceeded(String),
    // TODO: processing error? more detail?
    TemplateProcessingError(String),
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            TemplateError::AlreadyExists(ref template_id) => {
                write!(f, "Template already exists: {}", template_id)
            }
            TemplateError::UnknownTemplateId(ref template_id) => {
                write!(f, "Unknown template id: {}", template_id)
            }
            TemplateError::UnknownVersionedTemplateId(ref template_id) => {
                write!(f, "Unknown versioned template id: {}", template_id)
            }
            TemplateError::UnknownProjectId(ref project_id) => {
                write!(f, "Unknown project id: {}", project_id)
            }
            TemplateError::Internal(ref error) => write!(f, "Internal error: {}", error),
            TemplateError::IOError(ref error) => write!(f, "IO error: {}", error),
            TemplateError::Unauthorized(ref error) => write!(f, "Unauthorized: {}", error),
            TemplateError::LimitExceeded(ref error) => write!(f, "Limit exceeded: {}", error),
            TemplateError::TemplateProcessingError(ref error) => {
                write!(f, "Template processing error: {}", error)
            }
        }
    }
}

impl TemplateError {
    pub fn internal<T: Display>(error: T) -> Self {
        TemplateError::Internal(error.to_string())
    }

    pub fn unauthorized<T: Display>(error: T) -> Self {
        TemplateError::Unauthorized(error.to_string())
    }
}

impl From<RepoError> for TemplateError {
    fn from(error: RepoError) -> Self {
        TemplateError::internal(error)
    }
}

impl From<PlanLimitError> for TemplateError {
    fn from(error: PlanLimitError) -> Self {
        match error {
            PlanLimitError::Unauthorized(error) => TemplateError::Unauthorized(error),
            PlanLimitError::Internal(error) => TemplateError::Internal(error),
            PlanLimitError::AccountIdNotFound(_) => {
                TemplateError::Internal("Account not found".to_string())
            }
            PlanLimitError::ProjectIdNotFound(project_id) => {
                TemplateError::UnknownProjectId(project_id)
            }
            PlanLimitError::TemplateIdNotFound(template_id) => {
                TemplateError::UnknownTemplateId(template_id)
            }
        }
    }
}

impl From<ProjectError> for TemplateError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Unauthorized(error) => TemplateError::Unauthorized(error),
            ProjectError::Internal(error) => TemplateError::Internal(error),
            ProjectError::LimitExceeded(error) => TemplateError::LimitExceeded(error),
        }
    }
}

impl From<ProjectAuthorisationError> for TemplateError {
    fn from(error: ProjectAuthorisationError) -> Self {
        match error {
            ProjectAuthorisationError::Internal(error) => TemplateError::Internal(error),
            ProjectAuthorisationError::Unauthorized(error) => TemplateError::Unauthorized(error),
        }
    }
}

#[async_trait]
pub trait TemplateService {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        template_name: &TemplateName,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Template, TemplateError>;

    async fn update(
        &self,
        template_id: &TemplateId,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Template, TemplateError>;

    async fn download(
        &self,
        template_id: &TemplateId,
        version: Option<i32>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<u8>, TemplateError>;

    async fn get_protected_data(
        &self,
        template_id: &TemplateId,
        version: Option<i32>,
        auth: &AccountAuthorisation,
    ) -> Result<Option<Vec<u8>>, TemplateError>;

    async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        template_name: Option<TemplateName>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError>;

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError>;

    async fn get_by_version(
        &self,
        template_id: &VersionedTemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Template>, TemplateError>;

    async fn get_latest_version(
        &self,
        template_id: &TemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Template>, TemplateError>;

    async fn get(
        &self,
        template_id: &TemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError>;
}

pub struct TemplateServiceDefault {
    account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
    template_repo: Arc<dyn TemplateRepo + Sync + Send>,
    plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
    object_store: Arc<dyn TemplateObjectStore + Sync + Send>,
    project_service: Arc<dyn ProjectService + Sync + Send>,
    project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
}

impl TemplateServiceDefault {
    pub fn new(
        account_uploads_repo: Arc<dyn AccountUploadsRepo + Sync + Send>,
        template_repo: Arc<dyn TemplateRepo + Sync + Send>,
        plan_limit_service: Arc<dyn PlanLimitService + Sync + Send>,
        object_store: Arc<dyn TemplateObjectStore + Sync + Send>,
        project_service: Arc<dyn ProjectService + Sync + Send>,
        project_auth_service: Arc<dyn ProjectAuthorisationService + Sync + Send>,
    ) -> Self {
        TemplateServiceDefault {
            account_uploads_repo,
            template_repo,
            plan_limit_service,
            object_store,
            project_service,
            project_auth_service,
        }
    }
}

#[async_trait]
impl TemplateService for TemplateServiceDefault {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        template_name: &TemplateName,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Template, TemplateError> {
        let pid = project_id
            .clone()
            .map_or("default".to_string(), |n| n.0.to_string());
        let tn = template_name.0.clone();
        info!("Creating template for project {} with name {}", pid, tn);

        let project_id = if let Some(project_id) = project_id.clone() {
            project_id
        } else {
            let project = self.project_service.get_own_default(auth).await?;
            project.project_id
        };

        self.is_authorized_by_project(auth, &project_id, &ProjectAction::CreateTemplate)
            .await?;
        self.check_plan_limits(&project_id).await?;

        self.check_new_name(&project_id, template_name).await?;

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

        info!("create_template limits verified");

        self.validate_limits(storage_limit, upload_limit, &data)
            .await?;

        let metadata = self.process_template(&data)?;

        let template_id = TemplateId::new_v4();

        info!("Template {template_id} metadata: {metadata:?}");

        let versioned_template_id = VersionedTemplateId {
            template_id,
            version: 0,
        };

        let user_template_id = UserTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };
        let protected_template_id = ProtectedTemplateId {
            versioned_template_id: versioned_template_id.clone(),
        };

        info!("Pushing {:?}", user_template_id);

        let template_size: i32 = data.len().try_into().map_err(|e| {
            TemplateError::internal(format!("Failed to convert data length: {}", e))
        })?;

        tokio::try_join!(
            self.upload_user_template(&user_template_id, data.clone()),
            self.upload_protected_template(&protected_template_id, data)
        )?;

        info!("TemplateService create_template object store finished");

        let template = crate::model::Template {
            template_name: template_name.clone(),
            template_size,
            project_id: project_id.clone(),
            metadata,
            versioned_template_id,
            user_template_id,
            protected_template_id,
        };

        self.template_repo.upsert(&template.clone().into()).await?;

        info!("TemplateService create_template finished successfully");

        Ok(template)
    }

    async fn update(
        &self,
        template_id: &TemplateId,
        data: Vec<u8>,
        auth: &AccountAuthorisation,
    ) -> Result<crate::model::Template, TemplateError> {
        info!("Updating template {}", template_id.0);
        self.is_authorized_by_template(auth, template_id, &ProjectAction::UpdateTemplate)
            .await?;

        let plan_limit = self
            .plan_limit_service
            .get_template_limits(template_id)
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

        info!("update_template limits verified");

        self.validate_limits(storage_limit, upload_limit, &data)
            .await?;

        let metadata = self.process_template(&data)?;

        let next_template = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?
            .map(crate::model::Template::try_from)
            .transpose()
            .map_err(TemplateError::Internal)?
            .map(crate::model::Template::next_version)
            .ok_or(TemplateError::UnknownTemplateId(template_id.clone()))?;

        info!("Pushing {:?}", next_template.user_template_id);

        let template_size: i32 = data.len().try_into().map_err(|e| {
            TemplateError::internal(format!("Failed to convert data length: {}", e))
        })?;

        tokio::try_join!(
            self.upload_user_template(&next_template.user_template_id, data.clone()),
            self.upload_protected_template(&next_template.protected_template_id, data)
        )?;

        info!("TemplateService update_template object store finished");

        let template = crate::model::Template {
            template_size,
            metadata,
            ..next_template
        };

        self.template_repo.upsert(&template.clone().into()).await?;

        Ok(template)
    }

    async fn download(
        &self,
        template_id: &TemplateId,
        version: Option<i32>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<u8>, TemplateError> {
        self.is_authorized_by_template(auth, template_id, &ProjectAction::ViewTemplate)
            .await?;
        let versioned_template_id = {
            match version {
                Some(version) => VersionedTemplateId {
                    template_id: template_id.clone(),
                    version,
                },
                None => self
                    .template_repo
                    .get_latest_version(&template_id.0)
                    .await?
                    .map(crate::model::Template::try_from)
                    .transpose()
                    .map_err(TemplateError::Internal)?
                    .map(|t| t.versioned_template_id)
                    .ok_or(TemplateError::UnknownTemplateId(template_id.clone()))?,
            }
        };

        let id = ProtectedTemplateId {
            versioned_template_id,
        };

        self.object_store
            .get(&self.get_protected_object_store_key(&id))
            .await
            .tap_err(|e| error!("Error downloading template: {}", e))
            .map_err(|e| TemplateError::IOError(e.to_string()))
    }

    async fn get_protected_data(
        &self,
        template_id: &TemplateId,
        version: Option<i32>,
        auth: &AccountAuthorisation,
    ) -> Result<Option<Vec<u8>>, TemplateError> {
        info!(
            "Getting template  {} version {} data",
            template_id,
            version.map_or("N/A".to_string(), |v| v.to_string())
        );

        self.is_authorized_by_template(auth, template_id, &ProjectAction::ViewTemplate)
            .await?;

        let latest_template = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?;

        let v = match latest_template {
            Some(template) => match version {
                Some(v) if v <= template.version => v,
                None => template.version,
                _ => {
                    return Ok(None);
                }
            },
            None => {
                return Ok(None);
            }
        };

        let versioned_template_id = VersionedTemplateId {
            template_id: template_id.clone(),
            version: v,
        };

        let protected_id = ProtectedTemplateId {
            versioned_template_id,
        };

        let object_key = self.get_protected_object_store_key(&protected_id);

        let result = self
            .object_store
            .get(&object_key)
            .await
            .map_err(|e| TemplateError::internal(e.to_string()))?;

        Ok(Some(result))
    }

    async fn find_by_project_and_name(
        &self,
        project_id: Option<ProjectId>,
        template_name: Option<TemplateName>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError> {
        let pid = project_id
            .clone()
            .map_or("default".to_string(), |n| n.0.to_string());
        let tn = template_name.clone().map_or("N/A".to_string(), |n| n.0);
        info!("Getting template by project {} and name {}", pid, tn);

        let project_id = if let Some(project_id) = project_id.clone() {
            project_id
        } else {
            let project = self.project_service.get_own_default(auth).await?;
            project.project_id
        };

        self.is_authorized_by_project(auth, &project_id, &ProjectAction::ViewTemplate)
            .await?;

        let result = match template_name {
            Some(name) => {
                self.template_repo
                    .get_by_project_and_name(&project_id.0, &name.0)
                    .await?
            }
            None => self.template_repo.get_by_project(&project_id.0).await?,
        };

        result
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<_, String>>()
            .map_err(TemplateError::Internal)
    }

    async fn get_by_project(
        &self,
        project_id: &ProjectId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError> {
        info!("Getting template by project {}", project_id);
        self.is_authorized_by_project(auth, project_id, &ProjectAction::ViewTemplate)
            .await?;

        let result = self.template_repo.get_by_project(&project_id.0).await?;

        result
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<_, String>>()
            .map_err(TemplateError::Internal)
    }

    async fn get(
        &self,
        template_id: &TemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError> {
        info!("Getting template {}", template_id);
        self.is_authorized_by_template(auth, template_id, &ProjectAction::ViewTemplate)
            .await?;
        let result = self.template_repo.get(&template_id.0).await?;

        result
            .into_iter()
            .map(|t| t.try_into())
            .collect::<Result<_, String>>()
            .map_err(TemplateError::Internal)
    }

    async fn get_by_version(
        &self,
        template_id: &VersionedTemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Template>, TemplateError> {
        info!(
            "Getting template {} version {}",
            template_id.template_id, template_id.version
        );
        self.is_authorized_by_template(
            auth,
            &template_id.template_id,
            &ProjectAction::ViewTemplate,
        )
        .await?;
        let result = self
            .template_repo
            .get_by_version(&template_id.template_id.0, template_id.version)
            .await?;

        result
            .map(crate::model::Template::try_from)
            .transpose()
            .map_err(TemplateError::Internal)
    }

    async fn get_latest_version(
        &self,
        template_id: &TemplateId,
        auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Template>, TemplateError> {
        info!("Getting template {} latest version", template_id);
        self.is_authorized_by_template(auth, template_id, &ProjectAction::ViewTemplate)
            .await?;
        let result = self
            .template_repo
            .get_latest_version(&template_id.0)
            .await?;

        result
            .map(crate::model::Template::try_from)
            .transpose()
            .map_err(TemplateError::Internal)
    }
}

impl TemplateServiceDefault {
    async fn is_authorized_by_template(
        &self,
        auth: &AccountAuthorisation,
        template_id: &TemplateId,
        required_action: &ProjectAction,
    ) -> Result<(), TemplateError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            let permissions = self
                .project_auth_service
                .get_by_template(template_id, auth)
                .await
                .tap_err(|e| error!("Error getting template permissions: {:?}", e))?;

            if permissions.actions.contains(required_action) {
                Ok(())
            } else {
                Err(TemplateError::Unauthorized(format!(
                    "Account unauthorized to perform action on template {}: {}",
                    template_id.0, required_action
                )))
            }
        }
    }

    async fn is_authorized_by_project(
        &self,
        auth: &AccountAuthorisation,
        project_id: &ProjectId,
        required_action: &ProjectAction,
    ) -> Result<(), TemplateError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            let permissions = self
                .project_auth_service
                .get_by_project(project_id, auth)
                .await
                .tap_err(|e| error!("Error getting template permissions: {:?}", e))?;

            if permissions.actions.contains(required_action) {
                Ok(())
            } else {
                Err(TemplateError::Unauthorized(format!(
                    "Account unauthorized to perform action on project {}: {}",
                    project_id.0, required_action
                )))
            }
        }
    }

    async fn check_plan_limits(&self, project_id: &ProjectId) -> Result<(), TemplateError> {
        let limits = self
            .plan_limit_service
            .check_template_limit(project_id)
            .await
            .tap_err(|e| error!("Error checking limit for project {}: {:?}", project_id, e))?;

        if limits.not_in_limit() {
            Err(TemplateError::LimitExceeded(format!(
                "Template limit exceeded for project: {} (limit: {})",
                project_id.0, limits.limit
            )))
        } else {
            Ok(())
        }
    }

    async fn check_new_name(
        &self,
        project_id: &ProjectId,
        template_name: &TemplateName,
    ) -> Result<(), TemplateError> {
        let existing_templates = self
            .template_repo
            .get_by_project_and_name(&project_id.0, &template_name.0)
            .await
            .tap_err(|e| error!("Error getting existing templates: {}", e))?;

        existing_templates
            .into_iter()
            .next()
            .map(crate::model::Template::try_from)
            .transpose()
            .map_err(TemplateError::Internal)?
            .map_or(Ok(()), |t| {
                Err(TemplateError::AlreadyExists(
                    t.versioned_template_id.template_id,
                ))
            })
    }

    fn process_template(&self, data: &[u8]) -> Result<TemplateMetadata, TemplateError> {
        let component = Component::<IgnoreAllButMetadata>::from_bytes(data)
            .map_err(|e| TemplateError::TemplateProcessingError(e.to_string()))?;

        let producers = component
            .get_all_producers()
            .into_iter()
            .map(wasm_converter::convert_producers)
            .collect::<Vec<_>>();

        let state = AnalysisContext::new(component);

        let exports = state.get_top_level_exports().map_err(|e| {
            TemplateError::TemplateProcessingError(format!(
                "Error getting top level exports: {}",
                match e {
                    AnalysisFailure::Failed(e) => e,
                }
            ))
        })?;

        let exports = exports
            .into_iter()
            .map(wasm_converter::convert_export)
            .collect::<Vec<_>>();

        Ok(TemplateMetadata { exports, producers })
    }

    fn get_user_object_store_key(&self, id: &UserTemplateId) -> String {
        id.slug()
    }

    fn get_protected_object_store_key(&self, id: &ProtectedTemplateId) -> String {
        id.slug()
    }

    async fn upload_user_template(
        &self,
        user_template_id: &UserTemplateId,
        data: Vec<u8>,
    ) -> Result<(), TemplateError> {
        info!("Uploading user template: {:?}", user_template_id);

        self.object_store
            .put(&self.get_user_object_store_key(user_template_id), data)
            .await
            .map_err(|e| {
                let message = format!("Failed to upload user template: {}", e);
                error!("{}", message);
                TemplateError::IOError(message)
            })
    }

    async fn upload_protected_template(
        &self,
        protected_template_id: &ProtectedTemplateId,
        data: Vec<u8>,
    ) -> Result<(), TemplateError> {
        info!("Uploading protected template: {:?}", protected_template_id);

        self.object_store
            .put(
                &self.get_protected_object_store_key(protected_template_id),
                data,
            )
            .await
            .map_err(|e| {
                let message = format!("Failed to upload protected template: {}", e);
                error!("{}", message);
                TemplateError::IOError(message)
            })
    }

    async fn validate_limits(
        &self,
        storage_limit: CheckLimitResult,
        upload_limit: CheckLimitResult,
        data: &[u8],
    ) -> Result<(), TemplateError> {
        let num_bytes: i32 = data
            .len()
            .try_into()
            .map_err(|_| TemplateError::Internal("Failed to convert data length".into()))?;

        if num_bytes > 50000000 {
            Err(TemplateError::LimitExceeded(
                "Template size limit exceeded (limit: 50MB)".into(),
            ))
        } else if !storage_limit.add(num_bytes.into()).in_limit() {
            Err(TemplateError::LimitExceeded(format!(
                "Storage limit exceeded for account: {} (limit: {} MB)",
                storage_limit.account_id.value,
                storage_limit.limit / 1000000
            )))
        } else if !upload_limit.add(num_bytes.into()).in_limit() {
            Err(TemplateError::LimitExceeded(format!(
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
pub struct TemplateServiceNoOp {}

#[async_trait]
impl TemplateService for TemplateServiceNoOp {
    async fn create(
        &self,
        project_id: Option<ProjectId>,
        _template_name: &TemplateName,
        _data: Vec<u8>,
        _auth: &AccountAuthorisation,
    ) -> Result<crate::model::Template, TemplateError> {
        let fake_template = crate::model::Template {
            template_name: TemplateName("fake".to_string()),
            template_size: 0,
            project_id: project_id.unwrap_or(ProjectId::new_v4()),
            metadata: TemplateMetadata {
                exports: vec![],
                producers: vec![],
            },
            versioned_template_id: VersionedTemplateId {
                template_id: TemplateId::new_v4(),
                version: 0,
            },
            user_template_id: UserTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
            protected_template_id: ProtectedTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
        };

        Ok(fake_template)
    }

    async fn update(
        &self,
        _template_id: &TemplateId,
        _data: Vec<u8>,
        _auth: &AccountAuthorisation,
    ) -> Result<crate::model::Template, TemplateError> {
        let fake_template = crate::model::Template {
            template_name: TemplateName("fake".to_string()),
            template_size: 0,
            project_id: ProjectId::new_v4(),
            metadata: TemplateMetadata {
                exports: vec![],
                producers: vec![],
            },
            versioned_template_id: VersionedTemplateId {
                template_id: TemplateId::new_v4(),
                version: 0,
            },
            user_template_id: UserTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
            protected_template_id: ProtectedTemplateId {
                versioned_template_id: VersionedTemplateId {
                    template_id: TemplateId::new_v4(),
                    version: 0,
                },
            },
        };

        Ok(fake_template)
    }

    async fn download(
        &self,
        _template_id: &TemplateId,
        _version: Option<i32>,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<u8>, TemplateError> {
        Ok(vec![])
    }

    async fn get_protected_data(
        &self,
        _template_id: &TemplateId,
        _version: Option<i32>,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<Vec<u8>>, TemplateError> {
        Ok(None)
    }

    async fn find_by_project_and_name(
        &self,
        _project_id: Option<ProjectId>,
        _template_name: Option<TemplateName>,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError> {
        Ok(vec![])
    }

    async fn get_by_project(
        &self,
        _project_id: &ProjectId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError> {
        Ok(vec![])
    }

    async fn get_by_version(
        &self,
        _template_id: &VersionedTemplateId,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Template>, TemplateError> {
        Ok(None)
    }

    async fn get_latest_version(
        &self,
        _template_id: &TemplateId,
        _auth: &AccountAuthorisation,
    ) -> Result<Option<crate::model::Template>, TemplateError> {
        Ok(None)
    }

    async fn get(
        &self,
        _template_id: &TemplateId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<crate::model::Template>, TemplateError> {
        Ok(vec![])
    }
}

// Converters from golem_wasm_ast to crate model.
mod wasm_converter {
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance, AnalysedType,
    };
    use golem_wasm_ast::metadata::{Producers, ProducersField};

    // use cloud_common::model::*;
    use golem_service_base::model::*;

    pub fn convert_producers(producer: Producers) -> golem_service_base::model::Producers {
        golem_service_base::model::Producers {
            fields: producer
                .fields
                .into_iter()
                .map(convert_producer)
                .collect::<Vec<_>>(),
        }
    }

    fn convert_producer(producer: ProducersField) -> ProducerField {
        ProducerField {
            name: producer.name,
            values: producer
                .values
                .into_iter()
                .map(|value| golem_service_base::model::VersionedName {
                    name: value.name,
                    version: value.version,
                })
                .collect(),
        }
    }

    pub fn convert_export(export: AnalysedExport) -> Export {
        match export {
            AnalysedExport::Function(analysed_function) => {
                Export::Function(convert_function(analysed_function))
            }
            AnalysedExport::Instance(analysed_instance) => {
                Export::Instance(convert_instance(analysed_instance))
            }
        }
    }

    fn convert_function(analysed_function: AnalysedFunction) -> ExportFunction {
        ExportFunction {
            name: analysed_function.name,
            parameters: analysed_function
                .params
                .into_iter()
                .map(convert_function_param)
                .collect(),
            results: analysed_function
                .results
                .into_iter()
                .map(convert_function_result)
                .collect(),
        }
    }

    fn convert_instance(analysed_instance: AnalysedInstance) -> ExportInstance {
        ExportInstance {
            name: analysed_instance.name,
            functions: analysed_instance
                .funcs
                .into_iter()
                .map(convert_function)
                .collect(),
        }
    }

    fn convert_function_param(param: AnalysedFunctionParameter) -> FunctionParameter {
        FunctionParameter {
            name: param.name,
            typ: convert_type(param.typ),
        }
    }

    fn convert_function_result(result: AnalysedFunctionResult) -> FunctionResult {
        FunctionResult {
            name: result.name,
            typ: convert_type(result.typ),
        }
    }

    fn convert_type(analysed_type: AnalysedType) -> Type {
        match analysed_type {
            AnalysedType::Bool => Type::Bool(golem_service_base::model::TypeBool),
            AnalysedType::S8 => Type::S8(golem_service_base::model::TypeS8),
            AnalysedType::U8 => Type::U8(golem_service_base::model::TypeU8),
            AnalysedType::S16 => Type::S16(golem_service_base::model::TypeS16),
            AnalysedType::U16 => Type::U16(golem_service_base::model::TypeU16),
            AnalysedType::S32 => Type::S32(golem_service_base::model::TypeS32),
            AnalysedType::U32 => Type::U32(golem_service_base::model::TypeU32),
            AnalysedType::S64 => Type::S64(golem_service_base::model::TypeS64),
            AnalysedType::U64 => Type::U64(golem_service_base::model::TypeU64),
            AnalysedType::F32 => Type::F32(golem_service_base::model::TypeF32),
            AnalysedType::F64 => Type::F64(golem_service_base::model::TypeF64),
            AnalysedType::Chr => Type::Chr(golem_service_base::model::TypeChr),
            AnalysedType::Str => Type::Str(golem_service_base::model::TypeStr),
            AnalysedType::List(inner) => Type::List(golem_service_base::model::TypeList {
                inner: Box::new(convert_type(*inner)),
            }),
            AnalysedType::Tuple(items) => Type::Tuple(golem_service_base::model::TypeTuple {
                items: items.into_iter().map(convert_type).collect(),
            }),
            AnalysedType::Record(cases) => Type::Record(golem_service_base::model::TypeRecord {
                cases: cases
                    .into_iter()
                    .map(|(name, typ)| golem_service_base::model::NameTypePair {
                        name,
                        typ: Box::new(convert_type(typ)),
                    })
                    .collect(),
            }),
            AnalysedType::Flags(cases) => {
                Type::Flags(golem_service_base::model::TypeFlags { cases })
            }
            AnalysedType::Enum(cases) => Type::Enum(golem_service_base::model::TypeEnum { cases }),
            AnalysedType::Option(inner) => Type::Option(golem_service_base::model::TypeOption {
                inner: Box::new(convert_type(*inner)),
            }),
            AnalysedType::Result { ok, error } => {
                Type::Result(golem_service_base::model::TypeResult {
                    ok: ok.map(|t| Box::new(convert_type(*t))),
                    err: error.map(|t| Box::new(convert_type(*t))),
                })
            }
            AnalysedType::Variant(variants) => {
                Type::Variant(golem_service_base::model::TypeVariant {
                    cases: variants
                        .into_iter()
                        .map(
                            |(name, typ)| golem_service_base::model::NameOptionTypePair {
                                name,
                                typ: typ.map(|t| Box::new(convert_type(t))),
                            },
                        )
                        .collect(),
                })
            }
        }
    }
}
