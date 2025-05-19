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
use golem_common::config::DbPostgresConfig;
use golem_service_base::db;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use test_r::{inherit_test_dep, sequential};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;

inherit_test_dep!(Tracing);

#[sequential]
mod tests {
    use super::PostgresDb;
    use crate::Tracing;

    use crate::all::repo::UuidOwner;
    use golem_common::model::component::DefaultComponentOwner;
    use golem_common::model::plugin::{DefaultPluginOwner, DefaultPluginScope};
    use golem_component_service_base::repo::component::{
        ComponentRepo, DbComponentRepo, LoggedComponentRepo,
    };
    use golem_component_service_base::repo::plugin::{DbPluginRepo, LoggedPluginRepo, PluginRepo};
    use golem_service_base::repo::RepoError;
    use std::sync::Arc;
    use test_r::{inherit_test_dep, test, test_dep};

    inherit_test_dep!(Tracing);

    #[test_dep]
    async fn postgres_db_pool(_tracing: &Tracing) -> PostgresDb {
        PostgresDb::new().await
    }

    #[test_dep]
    fn postgres_component_repo(
        db: &PostgresDb,
    ) -> Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send> {
        Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
            db.pool.clone(),
        )))
    }

    #[test_dep]
    fn postgres_component_repo_uuid_owner(
        db: &PostgresDb,
    ) -> Arc<dyn ComponentRepo<UuidOwner> + Sync + Send> {
        Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
            db.pool.clone(),
        )))
    }

    #[test_dep]
    fn postgres_plugin_repo(
        db: &PostgresDb,
    ) -> Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync> {
        Arc::new(LoggedPluginRepo::new(DbPluginRepo::new(db.pool.clone())))
    }

    #[test]
    #[tracing::instrument]
    async fn repo_component_id_unique(
        component_repo: &Arc<dyn ComponentRepo<UuidOwner> + Sync + Send>,
    ) {
        crate::all::repo::test_repo_component_id_unique(component_repo.clone()).await
    }

    #[test]
    #[tracing::instrument]
    async fn repo_component_name_unique_in_namespace(
        component_repo: &Arc<dyn ComponentRepo<UuidOwner> + Sync + Send>,
    ) {
        crate::all::repo::test_repo_component_name_unique_in_namespace(component_repo.clone()).await
    }

    #[test]
    async fn repo_component_delete(
        component_repo: &Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send>,
    ) {
        crate::all::repo::test_repo_component_delete(component_repo.clone()).await
    }

    #[test]
    #[tracing::instrument]
    async fn repo_component_constraints(
        component_repo: &Arc<dyn ComponentRepo<UuidOwner> + Sync + Send>,
    ) {
        crate::all::repo::test_repo_component_constraints(component_repo.clone()).await
    }

    #[test]
    #[tracing::instrument]
    async fn default_plugin_repo(
        component_repo: &Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send>,
        plugin_repo: &Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
    ) -> Result<(), RepoError> {
        crate::all::repo::test_default_plugin_repo(component_repo.clone(), plugin_repo.clone())
            .await
    }

    #[test]
    #[tracing::instrument]
    async fn default_component_plugin_installation(
        component_repo: &Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send>,
        plugin_repo: &Arc<dyn PluginRepo<DefaultPluginOwner, DefaultPluginScope> + Send + Sync>,
    ) -> Result<(), RepoError> {
        crate::all::repo::test_default_component_plugin_installation(
            component_repo.clone(),
            plugin_repo.clone(),
        )
        .await
    }

    #[test]
    #[tracing::instrument]
    async fn component_find_by_names(
        component_repo: &Arc<dyn ComponentRepo<DefaultComponentOwner> + Sync + Send>,
    ) {
        crate::all::repo::test_repo_component_find_by_names(component_repo.clone()).await
    }
}

struct PostgresDb {
    _container: ContainerAsync<Postgres>,
    pub pool: PostgresPool,
}

impl PostgresDb {
    async fn new() -> Self {
        let (db_config, container) = Self::start_docker_postgres().await;

        db::postgres::migrate(
            &db_config,
            MigrationsDir::new("../golem-component-service/db/migration".into())
                .postgres_migrations(),
        )
        .await
        .unwrap();

        let pool = PostgresPool::configured(&db_config).await.unwrap();

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
