pub mod auth;
pub mod component;
pub mod limit;
pub mod project;

use golem_component_service_base::config::ComponentCompilationConfig;
use golem_component_service_base::repo::component::{ComponentRepo, DbComponentRepo};
use golem_component_service_base::service::component::{
    ComponentService as BaseComponentService,
    ComponentServiceDefault as BaseComponentServiceDefault,
};
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use golem_service_base::config::{ComponentStoreConfig, DbConfig};
use golem_service_base::db;
use golem_service_base::service::component_object_store;
use std::sync::Arc;

use crate::config::ComponentServiceConfig;
use crate::service::auth::{AuthService, CloudAuthService, CloudNamespace};
use crate::service::limit::{LimitService, LimitServiceDefault};
use crate::service::project::{ProjectService, ProjectServiceDefault};

#[derive(Clone)]
pub struct Services {
    pub component_service: Arc<dyn component::ComponentService + Sync + Send>,
    pub compilation_service: Arc<dyn ComponentCompilationService + Sync + Send>,
}

impl Services {
    pub async fn new(config: &ComponentServiceConfig) -> Result<Services, String> {
        let component_repo: Arc<dyn ComponentRepo + Sync + Send> = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(DbComponentRepo::new(db_pool.clone().into()))
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(DbComponentRepo::new(db_pool.clone().into()))
            }
        };

        let project_service: Arc<dyn ProjectService + Sync + Send> =
            Arc::new(ProjectServiceDefault::new(&config.cloud_service));

        let auth_service: Arc<dyn AuthService + Sync + Send> =
            Arc::new(CloudAuthService::new(project_service.clone()));

        let object_store: Arc<dyn component_object_store::ComponentObjectStore + Sync + Send> =
            match config.component_store.clone() {
                ComponentStoreConfig::S3(c) => {
                    Arc::new(component_object_store::AwsS3ComponentObjectStore::new(&c).await)
                }
                ComponentStoreConfig::Local(c) => {
                    Arc::new(component_object_store::FsComponentObjectStore::new(&c)?)
                }
            };

        let compilation_service: Arc<dyn ComponentCompilationService + Sync + Send> =
            match config.compilation.clone() {
                ComponentCompilationConfig::Enabled(config) => {
                    Arc::new(ComponentCompilationServiceDefault::new(config.uri()))
                }
                ComponentCompilationConfig::Disabled(_) => {
                    Arc::new(ComponentCompilationServiceDisabled)
                }
            };

        let limit_service: Arc<dyn LimitService + Sync + Send> =
            Arc::new(LimitServiceDefault::new(&config.cloud_service));

        let base_component_service: Arc<dyn BaseComponentService<CloudNamespace> + Sync + Send> =
            Arc::new(BaseComponentServiceDefault::new(
                component_repo.clone(),
                object_store.clone(),
                compilation_service.clone(),
            ));

        let component_service: Arc<dyn component::ComponentService + Sync + Send> =
            Arc::new(component::ComponentServiceDefault::new(
                base_component_service,
                auth_service.clone(),
                limit_service.clone(),
                project_service.clone(),
            ));

        Ok(Services {
            component_service,
            compilation_service,
        })
    }

    pub fn noop() -> Self {
        let component_service: Arc<dyn component::ComponentService + Sync + Send> =
            Arc::new(component::ComponentServiceNoop::default());

        let compilation_service: Arc<dyn ComponentCompilationService + Sync + Send> =
            Arc::new(ComponentCompilationServiceDisabled);

        Services {
            component_service,
            compilation_service,
        }
    }
}
