#[cfg(test)]
mod tests {
    use golem_service_base::config::{DbPostgresConfig, DbSqliteConfig};
    use golem_service_base::db;
    use golem_worker_service_base::api_definition::http::HttpApiDefinition;
    use golem_worker_service_base::api_definition::{
        ApiDefinitionId, ApiDeployment, ApiSite, ApiSiteString, ApiVersion,
    };
    use golem_worker_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
    use golem_worker_service_base::repo::{api_definition, api_deployment};
    use golem_worker_service_base::service::api_definition::{
        ApiDefinitionIdWithVersion, ApiDefinitionService, ApiDefinitionServiceDefault,
    };
    use golem_worker_service_base::service::api_deployment::{
        ApiDeploymentService, ApiDeploymentServiceDefault,
    };
    use golem_worker_service_base::service::component::{ComponentService, ComponentServiceNoop};
    use golem_worker_service_base::service::http::http_api_definition_validator::{
        HttpApiDefinitionValidator, RouteValidationError,
    };
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
                db_path: format!("/tmp/golem-worker-{}.db", uuid::Uuid::new_v4()),
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

        let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> =
            Arc::new(api_definition::DbApiDefinitionRepo::new(
                db_pool.clone().into(),
            ));
        let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
            Arc::new(api_deployment::DbApiDeploymentRepo::new(
                db_pool.clone().into(),
            ));

        test_services(api_definition_repo, api_deployment_repo).await;
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

        let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> =
            Arc::new(api_definition::DbApiDefinitionRepo::new(
                db_pool.clone().into(),
            ));
        let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
            Arc::new(api_deployment::DbApiDeploymentRepo::new(
                db_pool.clone().into(),
            ));

        test_services(api_definition_repo, api_deployment_repo).await;
    }

    async fn test_services(
        api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send>,
        api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send>,
    ) {
        let component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send> =
            Arc::new(ComponentServiceNoop {});

        let api_definition_validator_service = Arc::new(HttpApiDefinitionValidator {});

        let definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
                + Sync
                + Send,
        > = Arc::new(ApiDefinitionServiceDefault::new(
            component_service.clone(),
            api_definition_repo.clone(),
            api_definition_validator_service.clone(),
        ));

        let deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send> =
            Arc::new(ApiDeploymentServiceDefault::new(
                api_deployment_repo.clone(),
                api_definition_repo.clone(),
            ));

        let def1 = get_api_definition("def1", "0.0.1", "/api/get1", "worker1", "[]", "[]");
        let def2 = get_api_definition("def2", "0.0.1", "/api/get2", "worker2", "[]", "[]");
        let def3 = get_api_definition("def3", "0.0.1", "/api/get3", "worker3", "[]", "[]");
        let def4 = get_api_definition("def4", "0.0.1", "/api/get4", "worker4", "[]", "[]");
        let def5 = get_api_definition("def5", "0.0.1", "/api/get5", "worker5", "[]", "[]");
        let def5v2 = get_api_definition("def5", "0.0.2", "/api/get5/2", "worker5", "[]", "[]");

        definition_service
            .create(
                &def1,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        definition_service
            .create(
                &def2,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        definition_service
            .create(
                &def3,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        definition_service
            .create(
                &def4,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        definition_service
            .create(
                &def5,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        definition_service
            .create(
                &def5v2,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();

        let deployment = get_api_deployment("test.com", None, vec!["def1", "def2"]);
        deployment_service.deploy(&deployment).await.unwrap();

        let definitions = definition_service
            .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();
        assert_eq!(definitions.len(), 6);

        let definitions = definition_service
            .get_all_versions(
                &def5.id,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        assert_eq!(definitions.len(), 2);
        assert!(definitions.contains(&def5) && definitions.contains(&def5v2));

        let definitions = deployment_service
            .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
            .await
            .unwrap();

        assert_eq!(definitions.len(), 2);
        assert!(definitions.contains(&def1) && definitions.contains(&def2));

        let deployment = get_api_deployment("test.com", Some("my"), vec!["def4"]);
        deployment_service.deploy(&deployment).await.unwrap();

        let definitions = deployment_service
            .get_definitions_by_site(&ApiSiteString("my.test.com".to_string()))
            .await
            .unwrap();

        assert_eq!(definitions.len(), 1);
        assert!(definitions.contains(&def4));

        let deployment = get_api_deployment("test.com", None, vec!["def3"]);
        deployment_service.deploy(&deployment).await.unwrap();

        let definitions = deployment_service
            .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
            .await
            .unwrap();

        assert_eq!(definitions.len(), 3);
        assert!(
            definitions.contains(&def1)
                && definitions.contains(&def2)
                && definitions.contains(&def3)
        );
    }

    fn get_api_deployment(
        host: &str,
        subdomain: Option<&str>,
        definitions: Vec<&str>,
    ) -> ApiDeployment<DefaultNamespace> {
        let api_definition_keys: Vec<ApiDefinitionIdWithVersion> = definitions
            .into_iter()
            .map(|id| ApiDefinitionIdWithVersion {
                id: ApiDefinitionId(id.to_string()),
                version: ApiVersion("0.0.1".to_string()),
            })
            .collect();

        ApiDeployment {
            namespace: DefaultNamespace::default(),
            api_definition_keys,
            site: ApiSite {
                host: host.to_string(),
                subdomain: subdomain.map(|s| s.to_string()),
            },
        }
    }

    fn get_api_definition(
        id: &str,
        version: &str,
        path_pattern: &str,
        worker_id: &str,
        function_params: &str,
        response_mapping: &str,
    ) -> HttpApiDefinition {
        let yaml_string = format!(
            r#"
          id: {}
          version: {}
          routes:
          - method: Get
            path: {}
            binding:
              componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
              workerName: '{}'
              functionName: golem:it/api/get-cart-contents
              functionParams: {}
              response: '{}'
        "#,
            id, version, path_pattern, worker_id, function_params, response_mapping
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }
}
