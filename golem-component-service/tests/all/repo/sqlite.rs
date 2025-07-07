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

use crate::Tracing;
use golem_common::config::DbSqliteConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::{
    db,
    migration::{Migrations, MigrationsDir},
};
use test_r::{inherit_test_dep, sequential};
use uuid::Uuid;

inherit_test_dep!(Tracing);

#[sequential]
mod tests {
    use super::SqliteDb;
    use crate::Tracing;
    use golem_component_service::repo::component::{
        ComponentRepo, DbComponentRepo, LoggedComponentRepo,
    };
    use golem_component_service::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
    use golem_service_base::repo::RepoError;
    use std::sync::Arc;
    use test_r::{inherit_test_dep, test, test_dep};

    inherit_test_dep!(Tracing);

    #[test_dep]
    async fn db_pool(_tracing: &Tracing) -> SqliteDb {
        SqliteDb::new().await
    }

    #[test_dep]
    fn sqlite_component_repo(db: &SqliteDb) -> Arc<dyn ComponentRepo> {
        Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
            db.pool.clone(),
        )))
    }

    #[test_dep]
    fn sqlite_plugin_repo(db: &SqliteDb) -> Arc<dyn PluginRepo> {
        Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(db.pool.clone())))
    }

    #[test]
    #[tracing::instrument]
    async fn repo_component_id_unique(component_repo: &Arc<dyn ComponentRepo>) {
        crate::all::repo::test_repo_component_id_unique(component_repo.clone()).await
    }

    #[test]
    #[tracing::instrument]
    async fn repo_component_name_unique_in_namespace(component_repo: &Arc<dyn ComponentRepo>) {
        crate::all::repo::test_repo_component_name_unique_in_namespace(component_repo.clone()).await
    }

    #[test]
    #[tracing::instrument]
    async fn repo_component_delete(component_repo: &Arc<dyn ComponentRepo>) {
        crate::all::repo::test_repo_component_delete(component_repo.clone()).await
    }

    #[test]
    #[tracing::instrument]
    async fn repo_component_constraints(component_repo: &Arc<dyn ComponentRepo>) {
        crate::all::repo::test_repo_component_constraints(component_repo.clone()).await
    }

    #[test]
    #[tracing::instrument]
    async fn default_plugin_repo(
        component_repo: &Arc<dyn ComponentRepo>,
        plugin_repo: &Arc<dyn PluginRepo>,
    ) -> Result<(), RepoError> {
        crate::all::repo::test_default_plugin_repo(component_repo.clone(), plugin_repo.clone())
            .await
    }

    #[test]
    #[tracing::instrument]
    async fn component_find_by_names(component_repo: &Arc<dyn ComponentRepo>) {
        crate::all::repo::test_repo_component_find_by_names(component_repo.clone()).await
    }
}

pub struct SqliteDb {
    db_path: String,
    pub pool: SqlitePool,
}

impl SqliteDb {
    pub async fn new() -> Self {
        let db_path = format!("/tmp/golem-component-{}.db", Uuid::new_v4());
        let db_config = DbSqliteConfig {
            database: db_path.clone(),
            max_connections: 10,
        };

        db::sqlite::migrate(
            &db_config,
            MigrationsDir::new("../golem-component-service/db/migration".into())
                .sqlite_migrations(),
        )
        .await
        .unwrap();

        let pool = SqlitePool::configured(&db_config).await.unwrap();

        Self { db_path, pool }
    }
}

impl Drop for SqliteDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.db_path).unwrap();
    }
}
