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

use crate::api::dto::ApiMapper;
use crate::config::{ComponentCompilationConfig, ComponentServiceConfig};
use crate::repo::component::{ComponentRepo, DbComponentRepo, LoggedComponentRepo};
use crate::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
use crate::service::component::ComponentServiceDefault;
use crate::service::component::LazyComponentService;
use crate::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use crate::service::component_object_store::{
    BlobStorageComponentObjectStore, ComponentObjectStore,
};
use golem_common::config::DbConfig;
use golem_service_base::clients::auth::AuthService;
use golem_service_base::clients::limit::{LimitService, LimitServiceDefault};
use golem_service_base::clients::project::{ProjectService, ProjectServiceDefault};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::plugin_wasm_files::PluginWasmFilesService;
use golem_service_base::storage::blob::sqlite::SqliteBlobStorage;
use golem_service_base::storage::blob::BlobStorage;
use std::sync::Arc;
// use self::plugin::{PluginServiceDefault};
use crate::authed::component::AuthedComponentService;
use crate::authed::plugin::AuthedPluginService;
use crate::service::plugin::PluginService;
use crate::service::transformer_plugin_caller::TransformerPluginCallerDefault;

#[derive(Clone)]
pub struct Services {
    pub component_service: Arc<AuthedComponentService>,
    pub compilation_service: Arc<dyn ComponentCompilationService>,
    pub plugin_service: Arc<AuthedPluginService>,
    pub api_mapper: Arc<ApiMapper>,
}

impl Services {
    pub async fn new(config: &ComponentServiceConfig) -> Result<Services, String> {
        let blob_storage: Arc<dyn BlobStorage> = match &config.blob_storage {
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
                    .map_err(|e| format!("Failed to create sqlite pool: {e}"))?;
                Arc::new(SqliteBlobStorage::new(pool.clone()).await?)
            }
            BlobStorageConfig::InMemory(_) => {
                Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
            }
            _ => {
                return Err("Unsupported blob storage configuration".to_string());
            }
        };

        let (component_repo, plugin_repo) = match config.db.clone() {
            DbConfig::Postgres(config) => {
                let db_pool = PostgresPool::configured(&config)
                    .await
                    .map_err(|e| e.to_string())?;

                let component_repo: Arc<dyn ComponentRepo> = Arc::new(LoggedComponentRepo::new(
                    DbComponentRepo::new(db_pool.clone()),
                ));
                let plugin_repo: Arc<dyn PluginRepo> =
                    Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(db_pool.clone())));
                (component_repo, plugin_repo)
            }
            DbConfig::Sqlite(config) => {
                let db_pool = SqlitePool::configured(&config)
                    .await
                    .map_err(|e| e.to_string())?;

                let component_repo: Arc<dyn ComponentRepo> = Arc::new(LoggedComponentRepo::new(
                    DbComponentRepo::new(db_pool.clone()),
                ));
                let plugin_repo: Arc<dyn PluginRepo> =
                    Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(db_pool.clone())));
                (component_repo, plugin_repo)
            }
        };

        let project_service: Arc<dyn ProjectService> =
            Arc::new(ProjectServiceDefault::new(&config.cloud_service));

        let auth_service: Arc<AuthService> = Arc::new(AuthService::new(&config.cloud_service));

        let object_store: Arc<dyn ComponentObjectStore> =
            Arc::new(BlobStorageComponentObjectStore::new(blob_storage.clone()));

        let compilation_service: Arc<dyn ComponentCompilationService> =
            match config.compilation.clone() {
                ComponentCompilationConfig::Enabled(config) => {
                    Arc::new(ComponentCompilationServiceDefault::new(
                        config.uri(),
                        config.retries,
                        config.connect_timeout,
                    ))
                }
                ComponentCompilationConfig::Disabled(_) => {
                    Arc::new(ComponentCompilationServiceDisabled)
                }
            };

        let limit_service: Arc<dyn LimitService> =
            Arc::new(LimitServiceDefault::new(&config.cloud_service));

        let initial_component_files_service: Arc<InitialComponentFilesService> =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let plugin_wasm_files_service: Arc<PluginWasmFilesService> =
            Arc::new(PluginWasmFilesService::new(blob_storage.clone()));

        let component_service = Arc::new(LazyComponentService::new());

        let plugin_service: Arc<PluginService> = Arc::new(PluginService::new(
            plugin_repo.clone(),
            plugin_wasm_files_service.clone(),
            component_service.clone(),
        ));

        let transformer_plugin_caller = Arc::new(TransformerPluginCallerDefault::new(
            config.plugin_transformations.clone(),
        ));

        component_service
            .set_implementation(ComponentServiceDefault::new(
                component_repo.clone(),
                object_store.clone(),
                compilation_service.clone(),
                initial_component_files_service.clone(),
                plugin_service.clone(),
                plugin_wasm_files_service.clone(),
                transformer_plugin_caller.clone(),
                limit_service.clone(),
            ))
            .await;

        let authed_component_service: Arc<AuthedComponentService> =
            Arc::new(AuthedComponentService::new(
                component_service.clone(),
                auth_service.clone(),
                project_service.clone(),
            ));

        let authed_plugin_service: Arc<AuthedPluginService> = Arc::new(AuthedPluginService::new(
            plugin_service.clone(),
            auth_service.clone(),
            component_service,
        ));

        let api_mapper: Arc<ApiMapper> = Arc::new(ApiMapper::new(plugin_service.clone()));

        Ok(Services {
            component_service: authed_component_service,
            compilation_service,
            plugin_service: authed_plugin_service,
            api_mapper,
        })
    }
}
