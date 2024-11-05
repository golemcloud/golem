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

use crate::Tracing;
use golem_common::config::DbSqliteConfig;
use golem_service_base::db;
use sqlx::Pool;
use std::sync::Arc;
use test_r::{inherit_test_dep, sequential, test};
use uuid::Uuid;

inherit_test_dep!(Tracing);

#[sequential]
mod tests {
    use super::SqliteDb;
    use crate::Tracing;
    use golem_common::config::DbSqliteConfig;
    use golem_component_service_base::repo::component::{ComponentRepo, DbComponentRepo};
    use golem_service_base::db;
    use sqlx::Pool;
    use std::sync::Arc;

    use test_r::{inherit_test_dep, test, test_dep};
    use uuid::Uuid;

    inherit_test_dep!(Tracing);

    #[test_dep]
    async fn db_pool() -> SqliteDb {
        SqliteDb::new().await
    }

    #[test_dep]
    fn sqlite_component_repo(db: &SqliteDb) -> Arc<dyn ComponentRepo + Sync + Send> {
        Arc::new(DbComponentRepo::new(db.pool.clone()))
    }

    #[test]
    async fn repo_component_id_unique(component_repo: &Arc<dyn ComponentRepo + Sync + Send>) {
        crate::repo::test_repo_component_id_unique(component_repo.clone()).await
    }

    #[test]
    async fn repo_component_name_unique_in_namespace(
        component_repo: &Arc<dyn ComponentRepo + Sync + Send>,
    ) {
        crate::repo::test_repo_component_name_unique_in_namespace(component_repo.clone())
            .await
    }

    #[test]
    async fn repo_component_delete(component_repo: &Arc<dyn ComponentRepo + Sync + Send>) {
        crate::repo::test_repo_component_delete(component_repo.clone()).await
    }

    #[test]
    async fn repo_component_constraints(component_repo: &Arc<dyn ComponentRepo + Sync + Send>) {
        crate::repo::test_repo_component_constraints(component_repo.clone()).await
    }

    #[test]
    async fn component_constraint_incompatible_updates(
        component_repo: &Arc<dyn ComponentRepo + Sync + Send>,
    ) {
        crate::repo::test_component_constraint_incompatible_updates(
            component_repo.clone(),
        )
        .await
    }

    #[test]
    async fn services(component_repo: &Arc<dyn ComponentRepo + Sync + Send>) {
        crate::repo::test_services(component_repo.clone()).await
    }
}

struct SqliteDb {
    db_path: String,
    pub pool: Arc<Pool<sqlx::Sqlite>>,
}

impl SqliteDb {
    pub async fn new() -> Self {
        let db_path = format!("/tmp/golem-component-{}.db", Uuid::new_v4());
        let db_config = DbSqliteConfig {
            database: db_path.clone(),
            max_connections: 10,
        };

        db::sqlite_migrate(&db_config, "../golem-component-service/db/migration/sqlite")
            .await
            .unwrap();

        let pool = Arc::new(db::create_sqlite_pool(&db_config).await.unwrap());

        Self { db_path, pool }
    }
}

impl Drop for SqliteDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.db_path).unwrap();
    }
}
