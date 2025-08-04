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

use crate::repo::Deps;
use crate::Tracing;
use golem_common::config::DbSqliteConfig;
use golem_registry_service::repo::account::DbAccountRepo;
use golem_registry_service::repo::application::DbApplicationRepo;
use golem_registry_service::repo::component::DbComponentRepo;
use golem_registry_service::repo::environment::DbEnvironmentRepo;
use golem_registry_service::repo::http_api_definition::DbHttpApiDefinitionRepo;
use golem_registry_service::repo::http_api_deployment::DbHttpApiDeploymentRepo;
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::plan::DbPlanRepository;
use golem_service_base::db;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use test_r::test;
use test_r::{inherit_test_dep, test_dep};
use tracing::info;

inherit_test_dep!(Tracing);

// Deps setup --------------------------------------------------------------------------------------

pub struct SqliteDb {
    pub db_path: String,
    pub pool: SqlitePool,
}

impl SqliteDb {
    pub async fn new() -> Self {
        tempfile::tempfile().unwrap();
        let db_path = format!("/tmp/golem-registry-{}.db", new_repo_uuid());
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
        account_repo: Box::new(DbAccountRepo::logged(db.pool.clone())),
        application_repo: Box::new(DbApplicationRepo::logged(db.pool.clone())),
        environment_repo: Box::new(DbEnvironmentRepo::logged(db.pool.clone())),
        plan_repo: Box::new(DbPlanRepository::logged(db.pool.clone())),
        component_repo: Box::new(DbComponentRepo::logged(db.pool.clone())),
        http_api_definition_repo: Box::new(DbHttpApiDefinitionRepo::logged(db.pool.clone())),
        http_api_deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(db.pool.clone())),
        deployment_repo: Box::new(DbHttpApiDeploymentRepo::logged(db.pool.clone())),
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

#[test]
async fn test_component_create_and_get(deps: &Deps) {
    crate::repo::common::test_component_stage(deps).await;
}
