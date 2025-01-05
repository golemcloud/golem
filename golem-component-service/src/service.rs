// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::config::ComponentServiceConfig;
use golem_common::config::DbConfig;
use golem_common::model::component::DefaultComponentOwner;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_component_service_base::config::ComponentCompilationConfig;
use golem_component_service_base::config::ComponentStoreConfig;
use golem_component_service_base::repo::component::{
    ComponentRepo, DbComponentRepo, LoggedComponentRepo,
};
use golem_component_service_base::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
use golem_component_service_base::service::component::{ComponentService, ComponentServiceDefault};
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use golem_component_service_base::service::component_object_store;
use golem_component_service_base::service::component_object_store::{
    ComponentObjectStore, LoggedComponentObjectStore,
};
use golem_component_service_base::service::plugin::{PluginService, PluginServiceDefault};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::sqlite::SqlitePool;
use std::sync::Arc;

#[derive(Clone)]
pub struct Services {
    pub component_service: Arc<dyn ComponentService<DefaultComponentOwner> + Sync + Send>,
    pub compilation_service: Arc<dyn ComponentCompilationService + Sync + Send>,
    pub plugin_service:
        Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
}

impl Services {
    pub async fn new(config: &ComponentServiceConfig) -> Result<Services, String> {
        let (component_repo, plugin_repo) = match &config.db {
            DbConfig::Postgres(db_config) => {
                let db_pool = db::create_postgres_pool(db_config)
                    .await
                    .map_err(|e| e.to_string())?;

                let component_repo: Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send> =
                    Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                        db_pool.clone().into(),
                    )));
                let plugin_repo: Arc<
                    dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Sync + Send,
                > = Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(
                    db_pool.clone().into(),
                )));
                (component_repo, plugin_repo)
            }
            DbConfig::Sqlite(db_config) => {
                let db_pool = db::create_sqlite_pool(db_config)
                    .await
                    .map_err(|e| e.to_string())?;
                let component_repo: Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send> =
                    Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                        db_pool.clone().into(),
                    )));
                let plugin_repo: Arc<
                    dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Sync + Send,
                > = Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(
                    db_pool.clone().into(),
                )));
                (component_repo, plugin_repo)
            }
        };

        let blob_storage: Arc<dyn BlobStorage + Sync + Send> = match &config.blob_storage {
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

        let initial_component_files_service: Arc<InitialComponentFilesService> =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let object_store: Arc<dyn ComponentObjectStore + Sync + Send> =
            match &config.component_store {
                ComponentStoreConfig::S3(c) => {
                    let store: Arc<dyn ComponentObjectStore + Sync + Send> =
                        Arc::new(LoggedComponentObjectStore::new(
                            component_object_store::AwsS3ComponentObjectStore::new(c).await,
                        ));
                    store
                }
                ComponentStoreConfig::Local(c) => {
                    let store: Arc<dyn ComponentObjectStore + Sync + Send> =
                        Arc::new(LoggedComponentObjectStore::new(
                            component_object_store::FsComponentObjectStore::new(c)?,
                        ));
                    store
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

        let plugin_service: Arc<
            dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Sync + Send,
        > = Arc::new(PluginServiceDefault::new(plugin_repo));

        let component_service: Arc<dyn ComponentService<DefaultComponentOwner> + Sync + Send> =
            Arc::new(ComponentServiceDefault::new(
                component_repo.clone(),
                object_store.clone(),
                compilation_service.clone(),
                initial_component_files_service.clone(),
                plugin_service.clone(),
            ));

        Ok(Services {
            component_service,
            compilation_service,
            plugin_service,
        })
    }
}
