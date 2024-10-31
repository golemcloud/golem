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

use crate::config::ComponentServiceConfig;
use golem_common::config::DbConfig;
use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
use golem_component_service_base::config::ComponentCompilationConfig;
use golem_component_service_base::model::ComponentPluginInstallationTarget;
use golem_component_service_base::repo::component::{
    ComponentRepo, DbComponentRepo, LoggedComponentRepo,
};
use golem_component_service_base::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
use golem_component_service_base::repo::plugin_installation::{
    DbPluginInstallationRepo, LoggedPluginInstallationRepo, PluginInstallationRepo,
};
use golem_component_service_base::service::component::{ComponentService, ComponentServiceDefault};
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use golem_component_service_base::service::plugin::{PluginService, PluginServiceDefault};
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::config::ComponentStoreConfig;
use golem_service_base::db;
use golem_service_base::service::component_object_store;
use golem_service_base::service::component_object_store::{
    ComponentObjectStore, LoggedComponentObjectStore,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Services {
    pub component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send>,
    pub compilation_service: Arc<dyn ComponentCompilationService + Sync + Send>,
    pub plugin_service:
        Arc<dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
}

impl Services {
    pub async fn new(config: &ComponentServiceConfig) -> Result<Services, String> {
        // TODO: shared db pool for the repos

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

        let plugin_repo: Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Sync + Send> =
            match config.db.clone() {
                DbConfig::Postgres(c) => {
                    let db_pool = db::create_postgres_pool(&c)
                        .await
                        .map_err(|e| e.to_string())?;
                    Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(
                        db_pool.clone().into(),
                    )))
                }
                DbConfig::Sqlite(c) => {
                    let db_pool = db::create_sqlite_pool(&c)
                        .await
                        .map_err(|e| e.to_string())?;
                    Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(
                        db_pool.clone().into(),
                    )))
                }
            };

        let component_plugin_installations_repo: Arc<
            dyn PluginInstallationRepo<DefaultPluginOwner, ComponentPluginInstallationTarget>
                + Sync
                + Send,
        > = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(LoggedPluginInstallationRepo::new(
                    DbPluginInstallationRepo::new(db_pool.clone().into()),
                ))
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(LoggedPluginInstallationRepo::new(
                    DbPluginInstallationRepo::new(db_pool.clone().into()),
                ))
            }
        };

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
            ));

        let plugin_service: Arc<
            dyn PluginService<DefaultPluginOwner, DefaultPluginScope> + Sync + Send,
        > = Arc::new(PluginServiceDefault::new(
            plugin_repo,
            component_plugin_installations_repo,
            component_service.clone()
        ));

        Ok(Services {
            component_service,
            compilation_service,
            plugin_service,
        })
    }
}
