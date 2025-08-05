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
use crate::repo::Deps;
use golem_common::config::DbSqliteConfig;
use golem_registry_service::repo::account::{DbAccountRepo, LoggedAccountRepo};
use golem_registry_service::repo::application::{DbApplicationRepo, LoggedApplicationRepo};
use golem_registry_service::repo::environment::{DbEnvironmentRepo, LoggedEnvironmentRepo};
use golem_registry_service::repo::plan::{DbPlanRepository, LoggedPlanRepository};
use golem_service_base::db;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use std::sync::Arc;
use test_r::test;
use test_r::{inherit_test_dep, test_dep};
use tracing::info;
use uuid::Uuid;

inherit_test_dep!(Tracing);

// Deps setup --------------------------------------------------------------------------------------

pub struct SqliteDb {
    pub db_path: String,
    pub pool: SqlitePool,
}

impl SqliteDb {
    pub async fn new() -> Self {
        tempfile::tempfile().unwrap();
        let db_path = format!("/tmp/golem-registry-{}.db", Uuid::new_v4());
        let db_config = DbSqliteConfig {
            database: db_path.clone(),
            max_connections: 3,
            foreign_keys: true,
        };

        db::sqlite::migrate(
            &db_config,
            // NOTE: for now we are using the POSTGRES migrations (until they become incompatible)
            MigrationsDir::new("db/migration".into()).postgres_migrations(),
        )
        .await
        .unwrap();

        let pool = SqlitePool::configured(&db_config).await.unwrap();

        info!("Created sqlite database pool, database path: {}", db_path);

        Self { db_path, pool }
    }
}

impl Drop for SqliteDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.db_path).unwrap();
    }
}

#[test_dep]
async fn db_pool(_tracing: &Tracing) -> SqliteDb {
    SqliteDb::new().await
}

#[test_dep]
async fn deps(db: &SqliteDb) -> Deps {
    let deps = Deps {
        account_repo: Arc::new(LoggedAccountRepo::new(DbAccountRepo::new(db.pool.clone()))),
        application_repo: Arc::new(LoggedApplicationRepo::new(DbApplicationRepo::new(
            db.pool.clone(),
        ))),
        environment_repo: Arc::new(LoggedEnvironmentRepo::new(DbEnvironmentRepo::new(
            db.pool.clone(),
        ))),
        plan_repo: Arc::new(LoggedPlanRepository::new(DbPlanRepository::new(
            db.pool.clone(),
        ))),
    };
    deps.setup().await;
    deps
}

// Test cases --------------------------------------------------------------------------------------

#[test]
async fn test_create_and_get_account(deps: &Deps) {
    crate::repo::common::test_create_and_get_account(deps).await;
}

#[test]
async fn test_application_ensure(deps: &Deps) {
    crate::repo::common::test_application_ensure(deps).await;
}

#[test]
async fn test_application_ensure_concurrent(deps: &Deps) {
    crate::repo::common::test_application_ensure_concurrent(deps).await;
}

#[test]
async fn test_application_delete(deps: &Deps) {
    crate::repo::common::test_application_delete(deps).await;
}

#[test]
async fn test_environment_create(deps: &Deps) {
    crate::repo::common::test_environment_create(deps).await;
}

#[test]
async fn test_environment_create_concurrently(deps: &Deps) {
    crate::repo::common::test_environment_create_concurrently(deps).await;
}

#[test]
async fn test_environment_update(deps: &Deps) {
    crate::repo::common::test_environment_update(deps).await;
}

#[test]
async fn test_environment_update_concurrently(deps: &Deps) {
    crate::repo::common::test_environment_update_concurrently(deps).await;
}
