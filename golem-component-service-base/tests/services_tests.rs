#[cfg(test)]
mod tests {
    use golem_service_base::auth::DefaultNamespace;
    use golem_service_base::config::{ComponentStoreLocalConfig, DbPostgresConfig, DbSqliteConfig};
    use golem_service_base::db;

    use golem_common::model::ComponentId;
    use golem_component_service_base::repo::component::{ComponentRepo, DbComponentRepo};
    use golem_component_service_base::service::component::{
        ComponentService, ComponentServiceDefault,
    };
    use golem_component_service_base::service::component_compilation::{
        ComponentCompilationService, ComponentCompilationServiceDisabled,
    };
    use golem_service_base::model::ComponentName;
    use golem_service_base::service::component_object_store;
    use std::sync::Arc;
    use testcontainers::clients::Cli;
    use testcontainers::{Container, RunnableImage};
    use testcontainers_modules::postgres::Postgres;

    fn start_docker_postgres<'d>(docker: &'d Cli) -> (DbPostgresConfig, Container<'d, Postgres>) {
        let image = RunnableImage::from(Postgres::default()).with_tag("14.7-alpine");
        let container: Container<'d, Postgres> = docker.run(image);

        let config = DbPostgresConfig {
            host: "localhost".to_string(),
            port: container.get_host_port_ipv4(5432),
            database: "postgres".to_string(),
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            schema: Some("test".to_string()),
            max_connections: 10,
        };

        (config, container)
    }

    struct SqliteDb {
        db_path: String,
    }

    impl Default for SqliteDb {
        fn default() -> Self {
            Self {
                db_path: format!("/tmp/golem-component-{}.db", uuid::Uuid::new_v4()),
            }
        }
    }

    impl Drop for SqliteDb {
        fn drop(&mut self) {
            std::fs::remove_file(&self.db_path).unwrap();
        }
    }

    #[tokio::test]
    pub async fn test_with_postgres_db() {
        let cli = Cli::default();
        let (db_config, _container) = start_docker_postgres(&cli);

        db::postgres_migrate(&db_config, "tests/db/migration/postgres")
            .await
            .unwrap();

        let db_pool = db::create_postgres_pool(&db_config).await.unwrap();

        let component_repo: Arc<dyn ComponentRepo + Sync + Send> =
            Arc::new(DbComponentRepo::new(db_pool.clone().into()));

        test_services(component_repo).await;
    }

    #[tokio::test]
    pub async fn test_with_sqlite_db() {
        let db = SqliteDb::default();
        let db_config = DbSqliteConfig {
            database: db.db_path.clone(),
            max_connections: 10,
        };

        db::sqlite_migrate(&db_config, "tests/db/migration/sqlite")
            .await
            .unwrap();

        let db_pool = db::create_sqlite_pool(&db_config).await.unwrap();

        let component_repo: Arc<dyn ComponentRepo + Sync + Send> =
            Arc::new(DbComponentRepo::new(db_pool.clone().into()));

        test_services(component_repo).await;
    }

    async fn test_services(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
        let object_store: Arc<dyn component_object_store::ComponentObjectStore + Sync + Send> =
            Arc::new(
                component_object_store::FsComponentObjectStore::new(&ComponentStoreLocalConfig {
                    root_path: "/tmp/component".to_string(),
                    object_prefix: uuid::Uuid::new_v4().to_string(),
                })
                .unwrap(),
            );

        let compilation_service: Arc<dyn ComponentCompilationService + Sync + Send> =
            Arc::new(ComponentCompilationServiceDisabled);

        let component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send> =
            Arc::new(ComponentServiceDefault::new(
                component_repo.clone(),
                object_store.clone(),
                compilation_service.clone(),
            ));

        fn get_component_data(name: &str) -> Vec<u8> {
            let path = format!("../test-components/{}.wasm", name);
            std::fs::read(path).unwrap()
        }

        let component_name1 = ComponentName("shopping-cart".to_string());
        let component_name2 = ComponentName("rust-echo".to_string());

        let component1 = component_service
            .create(
                &ComponentId::new_v4(),
                &component_name1,
                get_component_data("shopping-cart"),
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();

        let component2 = component_service
            .create(
                &ComponentId::new_v4(),
                &component_name2,
                get_component_data("rust-echo"),
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();

        let component1_result = component_service
            .get_by_version(
                &component1.versioned_component_id,
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();
        assert!(component1_result.is_some());

        let component2_result = component_service
            .get_by_version(
                &component2.versioned_component_id,
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();
        assert!(component2_result.is_some());
        assert_eq!(component2_result.unwrap(), component2);

        let component1_result = component_service
            .get_latest_version(
                &component1.versioned_component_id.component_id,
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();
        assert!(component1_result.is_some());
        assert_eq!(component1_result.unwrap(), component1);

        let component1_result = component_service
            .get(
                &component1.versioned_component_id.component_id,
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();
        assert_eq!(component1_result.len(), 1);

        let component1v2 = component_service
            .update(
                &component1.versioned_component_id.component_id,
                get_component_data("shopping-cart"),
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();

        let component1_result = component_service
            .get_latest_version(
                &component1.versioned_component_id.component_id,
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();
        assert!(component1_result.is_some());
        assert_eq!(component1_result.unwrap(), component1v2);

        let component1_result = component_service
            .get(
                &component1.versioned_component_id.component_id,
                &DefaultNamespace::default(),
            )
            .await
            .unwrap();
        assert_eq!(component1_result.len(), 2);

        let component1_result = component_service
            .get_namespace(&component1.versioned_component_id.component_id)
            .await
            .unwrap();
        assert!(component1_result.is_some());
        assert_eq!(component1_result.unwrap(), DefaultNamespace::default());

        let component2_result = component_service
            .get_namespace(&component1.versioned_component_id.component_id)
            .await
            .unwrap();
        assert!(component2_result.is_some());
        assert_eq!(component2_result.unwrap(), DefaultNamespace::default());
    }
}
