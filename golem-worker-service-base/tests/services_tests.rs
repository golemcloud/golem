#[cfg(test)]
mod tests {
    use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
    use golem_service_base::config::{DbPostgresConfig, DbSqliteConfig};
    use golem_service_base::db;
    use golem_worker_service_base::api_definition::http::HttpApiDefinition;
    use golem_worker_service_base::api_definition::{
        ApiDefinitionId, ApiDeployment, ApiSite, ApiSiteString, ApiVersion,
    };
    use golem_worker_service_base::repo::{api_definition, api_deployment};
    use golem_worker_service_base::service::api_definition::{
        ApiDefinitionError, ApiDefinitionIdWithVersion, ApiDefinitionService,
        ApiDefinitionServiceDefault,
    };
    use golem_worker_service_base::service::api_deployment::{
        ApiDeploymentError, ApiDeploymentService, ApiDeploymentServiceDefault,
    };
    use golem_worker_service_base::service::component::{ComponentService, ComponentServiceNoop};
    use golem_worker_service_base::service::http::http_api_definition_validator::{
        HttpApiDefinitionValidator, RouteValidationError,
    };
    use std::sync::Arc;
    use testcontainers::clients::Cli;
    use testcontainers::{Container, RunnableImage};
    use testcontainers_modules::postgres::Postgres;
    use uuid::Uuid;

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

        db::postgres_migrate(&db_config, "../golem-worker-service/db/migration/postgres")
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

        db::sqlite_migrate(&db_config, "../golem-worker-service/db/migration/sqlite")
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
            api_deployment_repo.clone(),
            api_definition_validator_service.clone(),
        ));

        let deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send> =
            Arc::new(ApiDeploymentServiceDefault::new(
                api_deployment_repo.clone(),
                api_definition_repo.clone(),
            ));

        test_definition_crud(definition_service.clone()).await;
        test_deployment(definition_service.clone(), deployment_service.clone()).await;
        test_deployment_conflict(definition_service.clone(), deployment_service.clone()).await;
    }

    async fn test_deployment(
        definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
                + Sync
                + Send,
        >,
        deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send>,
    ) {
        let def1 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/1/foo/{user-id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.path.user-id}\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            false,
        );
        let def2draft = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/2/foo/{user-id}",
            "shopping-cart-${if (${request.body.user-id}>100) then 0 else 1}",
            "[ \"data\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            true,
        );
        let def2 = HttpApiDefinition {
            draft: false,
            ..def2draft.clone()
        };
        let def3 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/3/foo/{user-id}?{id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body}\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            false,
        );
        let def4 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/4/foo/{user-id}",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.path.user-id}\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            false,
        );

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
                &def2draft,
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

        let definitions = definition_service
            .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();
        assert_eq!(definitions.len(), 4);
        assert!(
            definitions.contains(&def2draft)
                && definitions.contains(&def1)
                && definitions.contains(&def3)
                && definitions.contains(&def4)
        );

        let deployment = get_api_deployment("test.com", None, vec![&def1.id.0, &def2.id.0]);
        deployment_service.deploy(&deployment).await.unwrap();

        let definitions = definition_service
            .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();
        assert_eq!(definitions.len(), 4);
        assert!(
            definitions.contains(&def2)
                && definitions.contains(&def1)
                && definitions.contains(&def3)
                && definitions.contains(&def4)
        );

        let definitions = deployment_service
            .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
            .await
            .unwrap();

        assert_eq!(definitions.len(), 2);
        assert!(definitions.contains(&def1) && definitions.contains(&def2));

        let deployment = get_api_deployment("test.com", Some("my"), vec![&def4.id.0]);
        deployment_service.deploy(&deployment).await.unwrap();

        let definitions = deployment_service
            .get_definitions_by_site(&ApiSiteString("my.test.com".to_string()))
            .await
            .unwrap();

        assert_eq!(definitions.len(), 1);
        assert!(definitions.contains(&def4));

        let deployment = get_api_deployment("test.com", None, vec![&def3.id.0]);
        deployment_service.deploy(&deployment).await.unwrap();

        let deployment = deployment_service
            .get_by_site(&ApiSiteString("test.com".to_string()))
            .await
            .unwrap();
        assert!(deployment.is_some());

        let deployments = deployment_service
            .get_by_id(&DefaultNamespace::default(), &def3.id)
            .await
            .unwrap();
        assert!(!deployments.is_empty());

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

        let deployment = get_api_deployment("test.com", None, vec![&def3.id.0]);
        deployment_service.undeploy(&deployment).await.unwrap();

        let definitions = deployment_service
            .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
            .await
            .unwrap();

        assert_eq!(definitions.len(), 2);
        assert!(definitions.contains(&def1) && definitions.contains(&def2));

        deployment_service
            .delete(
                &DefaultNamespace::default(),
                &ApiSiteString("test.com".to_string()),
            )
            .await
            .unwrap();

        let definitions = deployment_service
            .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
            .await
            .unwrap();
        assert!(definitions.is_empty());

        let deployment = deployment_service
            .get_by_site(&ApiSiteString("test.com".to_string()))
            .await
            .unwrap();
        assert!(deployment.is_none());
    }

    async fn test_deployment_conflict(
        definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
                + Sync
                + Send,
        >,
        deployment_service: Arc<dyn ApiDeploymentService<DefaultNamespace> + Sync + Send>,
    ) {
        let def1 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/get1",
            "worker1",
            "[]",
            "[]",
            false,
        );
        let def2 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/get2",
            "worker2",
            "[]",
            "[]",
            true,
        );

        let def3 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/get1",
            "worker2",
            "[]",
            "[]",
            false,
        );

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

        let deployment =
            get_api_deployment("test-conflict.com", None, vec![&def1.id.0, &def2.id.0]);
        deployment_service.deploy(&deployment).await.unwrap();

        let deployment = get_api_deployment("test-conflict.com", None, vec![&def3.id.0]);
        let deployment_result = deployment_service.deploy(&deployment).await;
        assert!(deployment_result.is_err());
        assert_eq!(
            deployment_result.unwrap_err().to_string(),
            ApiDeploymentError::<DefaultNamespace>::ApiDefinitionsConflict("/api/get1".to_string())
                .to_string()
        );

        let delete_result = definition_service
            .delete(
                &def1.id,
                &def1.version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await;
        assert!(delete_result.is_err());
        assert_eq!(
            delete_result.unwrap_err().to_string(),
            ApiDefinitionError::<RouteValidationError>::ApiDefinitionDeployed(
                "test-conflict.com".to_string()
            )
            .to_string()
        );
    }

    async fn test_definition_crud(
        definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
                + Sync
                + Send,
        >,
    ) {
        let def1v1 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/get1",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body.foo}\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            false,
        );
        let def1v1_upd = get_api_definition(
            &def1v1.id.0,
            "0.0.1",
            "/api/get1/1",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body.foo}\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            false,
        );
        let def1v2 = get_api_definition(
            &def1v1.id.0,
            "0.0.2",
            "/api/get1/2",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body.foo}\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            true,
        );

        let def1v2_upd = get_api_definition(
            &def1v1.id.0,
            "0.0.2",
            "/api/get1/22",
            "shopping-cart-${if (${request.path.user-id}>100) then 0 else 1}",
            "[\"${request.body.foo}\"]",
            "{status: if (worker.response.user == admin) then 401 else 200}",
            true,
        );

        definition_service
            .create(
                &def1v1,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        definition_service
            .create(
                &def1v2,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();

        let definitions = definition_service
            .get_all_versions(
                &def1v1.id,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        assert_eq!(definitions.len(), 2);
        assert!(definitions.contains(&def1v1) && definitions.contains(&def1v2));

        let update_result = definition_service
            .update(
                &def1v1_upd,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await;

        assert!(update_result.is_err());
        assert_eq!(
            update_result.unwrap_err().to_string(),
            ApiDefinitionError::<RouteValidationError>::ApiDefinitionNotDraft(def1v1_upd.id)
                .to_string()
        );

        let update_result = definition_service
            .update(
                &def1v2_upd,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await;
        assert!(update_result.is_ok());

        let definitions = definition_service
            .get_all_versions(
                &def1v1.id,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        assert_eq!(definitions.len(), 2);
        assert!(definitions.contains(&def1v1) && definitions.contains(&def1v2_upd));

        definition_service
            .delete(
                &def1v1.id,
                &def1v1.version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        definition_service
            .delete(
                &def1v2.id,
                &def1v2.version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();

        let definitions = definition_service
            .get_all_versions(
                &def1v1.id,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .unwrap();
        assert!(definitions.is_empty());
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
        draft: bool,
    ) -> HttpApiDefinition {
        let yaml_string = format!(
            r#"
          id: {}
          version: {}
          draft: {}
          routes:
          - method: Get
            path: {}
            binding:
              componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
              workerName: '{}'
              functionName: golem:it/api.{{get-cart-contents}}
              functionParams: {}
              response: '{}'
        "#,
            id, version, draft, path_pattern, worker_id, function_params, response_mapping
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }
}
