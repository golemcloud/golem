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
use golem_common::config::DbPostgresConfig;
use golem_service_base::db;
use sqlx::Pool;
use std::sync::Arc;
use test_r::{inherit_test_dep, sequential, test};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;

inherit_test_dep!(Tracing);

#[sequential]
mod tests {
    use super::PostgresDb;
    use crate::Tracing;
    use golem_common::config::DbPostgresConfig;
    use golem_component_service_base::repo::component::{ComponentRepo, DbComponentRepo};
    use golem_service_base::db;
    use sqlx::Pool;
    use std::sync::Arc;
    use test_r::{inherit_test_dep, test, test_dep};
    use testcontainers::runners::AsyncRunner;
    use testcontainers::{ContainerAsync, ImageExt};
    use testcontainers_modules::postgres::Postgres;

    inherit_test_dep!(Tracing);

    #[test_dep]
    async fn postgres_db_pool(_tracing: &Tracing) -> PostgresDb {
        PostgresDb::new().await
    }

    #[test_dep]
    fn postgres_component_repo(db: &PostgresDb) -> Arc<dyn ComponentRepo + Sync + Send> {
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

struct PostgresDb {
    _container: ContainerAsync<Postgres>,
    pub pool: Arc<Pool<sqlx::Postgres>>,
}

impl PostgresDb {
    async fn new() -> Self {
        let (db_config, container) = Self::start_docker_postgres().await;

        db::postgres_migrate(
            &db_config,
            "../golem-component-service/db/migration/postgres",
        )
        .await
        .unwrap();

        let pool = Arc::new(db::create_postgres_pool(&db_config).await.unwrap());

        Self {
            _container: container,
            pool,
        }
    }

    async fn start_docker_postgres() -> (DbPostgresConfig, ContainerAsync<Postgres>) {
        let container = Postgres::default()
            .with_tag("14.7-alpine")
            .start()
            .await
            .expect("Failed to start postgres container");

        let config = DbPostgresConfig {
            host: "localhost".to_string(),
            port: container
                .get_host_port_ipv4(5432)
                .await
                .expect("Failed to get port"),
            database: "postgres".to_string(),
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            schema: Some("test".to_string()),
            max_connections: 10,
        };

        (config, container)
    }
}
