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

pub mod component;

use golem_component_service_base::config::ComponentCompilationConfig;
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use golem_service_base::config::{ComponentStoreConfig, DbConfig};
use golem_service_base::db;
use golem_service_base::service::component_object_store;
use std::sync::Arc;

use crate::config::ComponentServiceConfig;
use crate::repo::component::{ComponentRepo, DbComponentRepo};

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

        let component_service: Arc<dyn component::ComponentService + Sync + Send> =
            Arc::new(component::ComponentServiceDefault::new(
                component_repo.clone(),
                object_store.clone(),
                compilation_service.clone(),
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
