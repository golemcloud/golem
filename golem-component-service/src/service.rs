// Copyright 2024 Golem Cloud
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

use golem_common::config::DbConfig;
use golem_component_service_base::config::ComponentCompilationConfig;
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use golem_service_base::config::ComponentStoreConfig;
use golem_service_base::db;
use golem_service_base::service::component_object_store;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::service::initial_component_files::{self, InitialComponentFilesService};
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::sqlite::SqlitePool;
use std::sync::Arc;

use crate::config::ComponentServiceConfig;
use golem_component_service_base::repo::component::{
    ComponentRepo, DbComponentRepo, LoggedComponentRepo,
};
use golem_component_service_base::service::component::{ComponentService, ComponentServiceDefault};
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::service::component_object_store::{
    ComponentObjectStore, LoggedComponentObjectStore,
};

#[derive(Clone)]
pub struct Services {
    pub component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send>,
    pub compilation_service: Arc<dyn ComponentCompilationService + Sync + Send>,
}

impl Services {
    pub async fn new(config: &ComponentServiceConfig) -> Result<Services, String> {
        let component_repo: Arc<dyn ComponentRepo + Sync + Send> = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                    db_pool.clone().into(),
                )))
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                    db_pool.clone().into(),
                )))
            }
        };

        let blob_storage: Arc<dyn BlobStorage + Sync + Send> = match &config.blob_storage {
            BlobStorageConfig::S3(config) => {
                Arc::new(golem_service_base::storage::blob::s3::S3BlobStorage::new(config.clone()).await)
            }
            BlobStorageConfig::LocalFileSystem(config) => {
                Arc::new(
                    golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                        .await?
                )
            }
            BlobStorageConfig::Sqlite(sqlite) => {
                let pool = SqlitePool::configured(&sqlite)
                    .await
                    .map_err(|e| format!("Failed to create sqlite pool: {}", e))?;
                Arc::new(
                    SqliteBlobStorage::new(pool.clone())
                        .await?,
                )
            }
            BlobStorageConfig::InMemory => {
                Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
            }
            _ => {
                return Err("Unsupported blob storage configuration".to_string());
            }
        };

        let initial_component_files_service: Arc<InitialComponentFilesService> =
            Arc::new(initial_component_files::InitialComponentFilesService::new(blob_storage.clone()));

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

        let component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send> =
            Arc::new(ComponentServiceDefault::new(
                component_repo.clone(),
                object_store.clone(),
                compilation_service.clone(),
                initial_component_files_service.clone(),
            ));

        Ok(Services {
            component_service,
            compilation_service,
        })
    }
}
