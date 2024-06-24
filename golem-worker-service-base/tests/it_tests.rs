#[cfg(test)]
mod tests {
    use golem_service_base::config::DbSqliteConfig;
    use golem_service_base::db;
    use golem_worker_service_base::api_definition::http::HttpApiDefinition;
    use golem_worker_service_base::api_definition::{
        ApiDefinitionId, ApiDeployment, ApiSite, ApiSiteString, ApiVersion,
    };
    use golem_worker_service_base::auth::{EmptyAuthCtx, EmptyNamespace};
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
    pub async fn test_sqlite_db() {
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
            Arc::new(api_definition::DbApiDefinitionRepoRepo::new(
                db_pool.clone().into(),
            ));
        let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
            Arc::new(api_deployment::DbApiDeploymentRepoRepo::new(
                db_pool.clone().into(),
            ));
        let component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send> =
            Arc::new(ComponentServiceNoop {});

        let api_definition_validator_service = Arc::new(HttpApiDefinitionValidator {});

        let definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, EmptyNamespace, RouteValidationError>
                + Sync
                + Send,
        > = Arc::new(ApiDefinitionServiceDefault::new(
            component_service.clone(),
            api_definition_repo.clone(),
            api_definition_validator_service.clone(),
        ));

        let deployment_service: Arc<dyn ApiDeploymentService<EmptyNamespace> + Sync + Send> =
            Arc::new(ApiDeploymentServiceDefault::new(
                api_deployment_repo.clone(),
                api_definition_repo.clone(),
            ));

        let def1 = get_api_definition("def1", "/api/get1", "worker1", "[]", "[]");
        let def2 = get_api_definition("def2", "/api/get2", "worker2", "[]", "[]");
        let def3 = get_api_definition("def3", "/api/get3", "worker3", "[]", "[]");
        let def4 = get_api_definition("def4", "/api/get4", "worker4", "[]", "[]");
        let def5 = get_api_definition("def5", "/api/get5", "worker5", "[]", "[]");

        definition_service
            .create(&def1, &EmptyNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();
        definition_service
            .create(&def2, &EmptyNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();
        definition_service
            .create(&def3, &EmptyNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();
        definition_service
            .create(&def4, &EmptyNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();
        definition_service
            .create(&def5, &EmptyNamespace::default(), &EmptyAuthCtx::default())
            .await
            .unwrap();

        let deployment = get_api_deployment("test.com", None, vec!["def1", "def2"]);
        deployment_service.deploy(&deployment).await.unwrap();

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
    ) -> ApiDeployment<EmptyNamespace> {
        let api_definition_keys: Vec<ApiDefinitionIdWithVersion> = definitions
            .into_iter()
            .map(|id| ApiDefinitionIdWithVersion {
                id: ApiDefinitionId(id.to_string()),
                version: ApiVersion("0.0.1".to_string()),
            })
            .collect();

        ApiDeployment {
            namespace: EmptyNamespace::default(),
            api_definition_keys,
            site: ApiSite {
                host: host.to_string(),
                subdomain: subdomain.map(|s| s.to_string()),
            },
        }
    }

    fn get_api_definition(
        id: &str,
        path_pattern: &str,
        worker_id: &str,
        function_params: &str,
        response_mapping: &str,
    ) -> HttpApiDefinition {
        let yaml_string = format!(
            r#"
          id: {}
          version: 0.0.1
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
            id, path_pattern, worker_id, function_params, response_mapping
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }
}
