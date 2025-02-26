use crate::config::ComponentServiceConfig;
use crate::model::CloudPluginScope;
use crate::service::component::CloudComponentService;
use crate::service::plugin::CloudPluginService;
use cloud_api_grpc::proto::golem::cloud::project::v1::project_error;
use cloud_common::clients::auth::{AuthServiceError, BaseAuthService, CloudAuthService};
use cloud_common::clients::grant::{GrantService, GrantServiceDefault};
use cloud_common::clients::limit::{LimitError, LimitService, LimitServiceDefault};
use cloud_common::clients::project::{ProjectError, ProjectService, ProjectServiceDefault};
use cloud_common::model::{CloudComponentOwner, CloudPluginOwner};
use golem_common::config::DbConfig;
use golem_common::SafeDisplay;
use golem_component_service_base::config::{ComponentCompilationConfig, ComponentStoreConfig};
use golem_component_service_base::repo::component::{
    ComponentRepo, DbComponentRepo, LoggedComponentRepo,
};
use golem_component_service_base::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
use golem_component_service_base::service::component::ComponentServiceDefault as BaseComponentServiceDefault;
use golem_component_service_base::service::component::{
    ComponentError as BaseComponentError, ComponentService as BaseComponentService,
};
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use golem_component_service_base::service::component_object_store;
use golem_component_service_base::service::plugin::{
    PluginError, PluginService, PluginServiceDefault,
};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db;
use golem_service_base::repo::RepoError;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::sqlite::SqlitePool;
use std::fmt::Display;
use std::sync::Arc;

pub mod component;
pub mod plugin;

#[derive(Clone)]
pub struct Services {
    pub component_service: Arc<CloudComponentService>,
    pub compilation_service: Arc<dyn ComponentCompilationService + Send + Sync>,
    pub plugin_service: Arc<CloudPluginService>,
}

impl Services {
    pub async fn new(config: &ComponentServiceConfig) -> Result<Services, String> {
        let blob_storage: Arc<dyn BlobStorage + Send + Sync> = match &config.blob_storage {
            BlobStorageConfig::S3(config) => Arc::new(
                golem_service_base::storage::blob::s3::S3BlobStorage::new(config.clone()).await,
            ),
            BlobStorageConfig::LocalFileSystem(config) => Arc::new(
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await?,
            ),
            BlobStorageConfig::Sqlite(sqlite) => {
                let pool = SqlitePool::configured(sqlite)
                    .await
                    .map_err(|e| format!("Failed to create sqlite pool: {}", e))?;
                Arc::new(SqliteBlobStorage::new(pool.clone()).await?)
            }
            BlobStorageConfig::InMemory => {
                Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
            }
            _ => {
                return Err("Unsupported blob storage configuration".to_string());
            }
        };

        let (component_repo, plugin_repo) = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;

                let component_repo: Arc<dyn ComponentRepo<CloudComponentOwner> + Send + Sync> =
                    Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                        db_pool.clone().into(),
                    )));
                let plugin_repo: Arc<
                    dyn PluginRepo<CloudPluginOwner, CloudPluginScope> + Send + Sync,
                > = Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(
                    db_pool.clone().into(),
                )));
                (component_repo, plugin_repo)
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;

                let component_repo: Arc<dyn ComponentRepo<CloudComponentOwner> + Send + Sync> =
                    Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                        db_pool.clone().into(),
                    )));
                let plugin_repo: Arc<
                    dyn PluginRepo<CloudPluginOwner, CloudPluginScope> + Send + Sync,
                > = Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(
                    db_pool.clone().into(),
                )));
                (component_repo, plugin_repo)
            }
        };

        let project_service: Arc<dyn ProjectService + Send + Sync> =
            Arc::new(ProjectServiceDefault::new(&config.cloud_service));

        let grant_service: Arc<dyn GrantService + Send + Sync> =
            Arc::new(GrantServiceDefault::new(&config.cloud_service));

        let auth_service: Arc<dyn BaseAuthService + Send + Sync> = Arc::new(CloudAuthService::new(
            project_service.clone(),
            grant_service.clone(),
        ));

        let object_store: Arc<dyn component_object_store::ComponentObjectStore + Send + Sync> =
            match config.component_store.clone() {
                ComponentStoreConfig::S3(c) => {
                    Arc::new(component_object_store::AwsS3ComponentObjectStore::new(&c).await)
                }
                ComponentStoreConfig::Local(c) => {
                    Arc::new(component_object_store::FsComponentObjectStore::new(&c)?)
                }
            };

        let compilation_service: Arc<dyn ComponentCompilationService + Send + Sync> =
            match config.compilation.clone() {
                ComponentCompilationConfig::Enabled(config) => {
                    Arc::new(ComponentCompilationServiceDefault::new(config.uri()))
                }
                ComponentCompilationConfig::Disabled(_) => {
                    Arc::new(ComponentCompilationServiceDisabled)
                }
            };

        let limit_service: Arc<dyn LimitService + Send + Sync> =
            Arc::new(LimitServiceDefault::new(&config.cloud_service));

        let initial_component_files_service: Arc<InitialComponentFilesService> =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let base_plugin_service: Arc<
            dyn PluginService<CloudPluginOwner, CloudPluginScope> + Send + Sync,
        > = Arc::new(PluginServiceDefault::new(plugin_repo.clone()));

        let base_component_service: Arc<
            dyn BaseComponentService<CloudComponentOwner> + Send + Sync,
        > = Arc::new(BaseComponentServiceDefault::new(
            component_repo.clone(),
            object_store.clone(),
            compilation_service.clone(),
            initial_component_files_service.clone(),
            base_plugin_service.clone(),
        ));

        let component_service: Arc<CloudComponentService> = Arc::new(CloudComponentService::new(
            base_component_service,
            auth_service.clone(),
            limit_service.clone(),
            project_service.clone(),
            base_plugin_service.clone(),
        ));

        let plugin_service: Arc<CloudPluginService> = Arc::new(CloudPluginService::new(
            base_plugin_service.clone(),
            component_service.clone(),
            auth_service.clone(),
        ));

        Ok(Services {
            component_service,
            compilation_service,
            plugin_service,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloudComponentError {
    #[error("Unknown project: {0}")]
    UnknownProject(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Limit exceeded: {0}")]
    LimitExceeded(String),
    #[error(transparent)]
    BaseComponentError(#[from] BaseComponentError),
    #[error(transparent)]
    BasePluginError(#[from] PluginError),
    #[error(transparent)]
    InternalAuthServiceError(AuthServiceError),
    #[error(transparent)]
    InternalLimitError(LimitError),
    #[error(transparent)]
    InternalProjectError(ProjectError),
}

impl SafeDisplay for CloudComponentError {
    fn to_safe_string(&self) -> String {
        match self {
            CloudComponentError::Unauthorized(_) => self.to_string(),
            CloudComponentError::LimitExceeded(_) => self.to_string(),
            CloudComponentError::BaseComponentError(inner) => inner.to_safe_string(),
            CloudComponentError::InternalAuthServiceError(inner) => inner.to_safe_string(),
            CloudComponentError::InternalLimitError(inner) => inner.to_safe_string(),
            CloudComponentError::InternalProjectError(inner) => inner.to_safe_string(),
            CloudComponentError::UnknownProject(_) => self.to_string(),
            CloudComponentError::BasePluginError(inner) => inner.to_safe_string(),
        }
    }
}

impl CloudComponentError {
    pub fn unauthorized<T: Display>(error: T) -> Self {
        CloudComponentError::Unauthorized(error.to_string())
    }
}

impl From<RepoError> for CloudComponentError {
    fn from(error: RepoError) -> Self {
        CloudComponentError::BaseComponentError(BaseComponentError::InternalRepoError(error))
    }
}

impl From<AuthServiceError> for CloudComponentError {
    fn from(error: AuthServiceError) -> Self {
        match error {
            AuthServiceError::Unauthorized(error) => CloudComponentError::Unauthorized(error),
            AuthServiceError::Forbidden(error) => CloudComponentError::Unauthorized(error),
            _ => CloudComponentError::InternalAuthServiceError(error),
        }
    }
}

impl From<LimitError> for CloudComponentError {
    fn from(error: LimitError) -> Self {
        match error {
            LimitError::Unauthorized(string) => CloudComponentError::Unauthorized(string),
            LimitError::LimitExceeded(string) => CloudComponentError::LimitExceeded(string),
            _ => CloudComponentError::InternalLimitError(error),
        }
    }
}

impl From<ProjectError> for CloudComponentError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Server(
                cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError {
                    error: Some(project_error::Error::Unauthorized(e)),
                },
            ) => CloudComponentError::Unauthorized(e.error),
            ProjectError::Server(
                cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError {
                    error: Some(project_error::Error::LimitExceeded(e)),
                },
            ) => CloudComponentError::LimitExceeded(e.error),
            ProjectError::Server(
                cloud_api_grpc::proto::golem::cloud::project::v1::ProjectError {
                    error: Some(project_error::Error::NotFound(e)),
                },
            ) => CloudComponentError::UnknownProject(e.error),
            _ => CloudComponentError::InternalProjectError(error),
        }
    }
}

impl From<CloudComponentError> for PluginError {
    fn from(value: CloudComponentError) -> Self {
        match value {
            CloudComponentError::BaseComponentError(inner) => {
                PluginError::InternalComponentError(inner)
            }
            _ => PluginError::FailedToGetAvailableScopes {
                error: value.to_safe_string(),
            },
        }
    }
}
