use test_r::test;

use async_trait::async_trait;
use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_common::model::ComponentId;
use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_service_base::db;
use golem_service_base::model::Component;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedInstance,
};
use golem_worker_service_base::api_definition::http::HttpApiDefinition;
use golem_worker_service_base::api_definition::http::HttpApiDefinitionRequest;
use golem_worker_service_base::api_definition::{
    ApiDefinitionId, ApiDeploymentRequest, ApiSite, ApiSiteString, ApiVersion,
};
use golem_worker_service_base::repo::{api_definition, api_deployment};
use golem_worker_service_base::service::api_definition::{
    ApiDefinitionError, ApiDefinitionIdWithVersion, ApiDefinitionService,
    ApiDefinitionServiceDefault,
};
use golem_worker_service_base::service::api_deployment::{
    ApiDeploymentError, ApiDeploymentService, ApiDeploymentServiceDefault,
};
use golem_worker_service_base::service::component::{ComponentResult, ComponentService};
use golem_worker_service_base::service::http::http_api_definition_validator::{
    HttpApiDefinitionValidator, RouteValidationError,
};

use chrono::Utc;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_wasm_ast::analysis::analysed_type::str;
use std::sync::Arc;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

test_r::enable!();

async fn start_docker_postgres() -> (DbPostgresConfig, ContainerAsync<Postgres>) {
    let image = Postgres::default().with_tag("14.7-alpine");
    let container = image
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

struct SqliteDb {
    db_path: String,
}

impl Default for SqliteDb {
    fn default() -> Self {
        Self {
            db_path: format!("/tmp/golem-worker-{}.db", Uuid::new_v4()),
        }
    }
}

impl Drop for SqliteDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.db_path).unwrap();
    }
}

#[test]
pub async fn test_with_postgres_db() {
    let (db_config, _container) = start_docker_postgres().await;

    db::postgres_migrate(&db_config, "../golem-worker-service/db/migration/postgres")
        .await
        .unwrap();

    let db_pool = db::create_postgres_pool(&db_config).await.unwrap();

    let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> = Arc::new(
        api_definition::DbApiDefinitionRepo::new(db_pool.clone().into()),
    );
    let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> = Arc::new(
        api_deployment::DbApiDeploymentRepo::new(db_pool.clone().into()),
    );

    test_services(api_definition_repo, api_deployment_repo).await;
}

#[test]
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

    let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> = Arc::new(
        api_definition::DbApiDefinitionRepo::new(db_pool.clone().into()),
    );
    let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> = Arc::new(
        api_deployment::DbApiDeploymentRepo::new(db_pool.clone().into()),
    );

    test_services(api_definition_repo, api_deployment_repo).await;
}

struct TestComponentService;

impl TestComponentService {
    pub fn test_component() -> Component {
        use golem_common::model::component_metadata::ComponentMetadata;
        use golem_service_base::model::{ComponentName, VersionedComponentId};

        let id = VersionedComponentId {
            component_id: ComponentId::try_from("0b6d9cd8-f373-4e29-8a5a-548e61b868a5").unwrap(),
            version: 0,
        };

        Component {
            versioned_component_id: id.clone(),
            component_name: ComponentName("test".to_string()),
            component_size: 0,
            metadata: ComponentMetadata {
                exports: Self::get_metadata(),
                producers: vec![],
                memories: vec![],
            },
            created_at: Some(Utc::now()),
            component_type: None,
        }
    }

    fn get_metadata() -> Vec<AnalysedExport> {
        let analysed_export = AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![AnalysedFunction {
                name: "get-cart-contents".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "a".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: str(),
                }],
            }],
        });

        vec![analysed_export]
    }
}

#[async_trait]
impl<AuthCtx> ComponentService<AuthCtx> for TestComponentService {
    async fn get_by_version(
        &self,
        _component_id: &ComponentId,
        _version: u64,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component> {
        Ok(Self::test_component())
    }

    async fn get_latest(
        &self,
        _component_id: &ComponentId,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component> {
        Ok(Self::test_component())
    }

    async fn create_or_update_constraints(
        &self,
        _component_id: &ComponentId,
        _constraints: FunctionConstraintCollection,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraintCollection> {
        Ok(FunctionConstraintCollection {
            function_constraints: vec![],
        })
    }
}

async fn test_services(
    api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send>,
    api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send>,
) {
    let component_service: Arc<dyn ComponentService<EmptyAuthCtx> + Sync + Send> =
        Arc::new(TestComponentService {});

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

    let deployment_service: Arc<
        dyn ApiDeploymentService<EmptyAuthCtx, DefaultNamespace> + Sync + Send,
    > = Arc::new(ApiDeploymentServiceDefault::new(
        api_deployment_repo.clone(),
        api_definition_repo.clone(),
        component_service.clone(),
    ));

    test_definition_crud(definition_service.clone()).await;
    test_delete_non_existing(definition_service.clone()).await;
    test_deployment(definition_service.clone(), deployment_service.clone()).await;
    test_deployment_conflict(definition_service.clone(), deployment_service.clone()).await;
}

async fn test_deployment(
    definition_service: Arc<
        dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
            + Sync
            + Send,
    >,
    deployment_service: Arc<dyn ApiDeploymentService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
) {
    let def1 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/1/foo/{user-id}",
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
            false,
        );
    let def2draft = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/2/foo/{user-id}",
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
            true,
        );
    let def2 = HttpApiDefinitionRequest {
        draft: false,
        ..def2draft.clone()
    };
    let def3 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/3/foo/{user-id}?{id}",
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
            false,
        );
    let def4 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/4/foo/{user-id}",
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
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

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 4);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2draft.clone(), def3.clone(), def4.clone()]
    ));

    let deployment = get_api_deployment("test.com", None, vec![&def1.id.0, &def2.id.0]);
    deployment_service
        .deploy(&deployment, &EmptyAuthCtx::default())
        .await
        .unwrap();

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all(&DefaultNamespace::default(), &EmptyAuthCtx::default())
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 4);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone(), def3.clone(), def4.clone()]
    ));

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone()]
    ));

    let deployment = get_api_deployment("test.com", Some("my"), vec![&def4.id.0]);
    deployment_service
        .deploy(&deployment, &EmptyAuthCtx::default())
        .await
        .unwrap();

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&ApiSiteString("my.test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 1);
    assert!(contains_definitions(definitions, vec![def4.clone()]));

    let deployment = get_api_deployment("test.com", None, vec![&def3.id.0]);
    deployment_service
        .deploy(&deployment, &EmptyAuthCtx::default())
        .await
        .unwrap();

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

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 3);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone(), def3.clone()]
    ));

    let deployment = get_api_deployment("test.com", None, vec![&def3.id.0]);
    deployment_service.undeploy(&deployment).await.unwrap();

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&ApiSiteString("test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone()]
    ));

    assert!(
        deployment_service
            .delete(
                &DefaultNamespace::default(),
                &ApiSiteString("test.com".to_string()),
            )
            .await
            .is_ok(),
        "Deployment not found"
    );

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

    let definition1 = definition_service
        .get(
            &def1.id,
            &def1.version,
            &DefaultNamespace::default(),
            &EmptyAuthCtx::default(),
        )
        .await
        .unwrap();
    assert!(definition1.is_some_and(|x| x.draft));

    let definition2 = definition_service
        .get(
            &def2.id,
            &def2.version,
            &DefaultNamespace::default(),
            &EmptyAuthCtx::default(),
        )
        .await
        .unwrap();
    assert!(definition2.is_some_and(|x| x.draft));
}

async fn test_deployment_conflict(
    definition_service: Arc<
        dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
            + Sync
            + Send,
    >,
    deployment_service: Arc<dyn ApiDeploymentService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
) {
    let def1 = get_api_definition(
            &Uuid::new_v4().to_string(),
            "0.0.1",
            "/api/get1",
            "\"worker1\"",
            "${ let status: u64 = 200; { headers: { ContentType: \"json\", userid: \"foo\"}, body: golem:it/api.{get-cart-contents}(\"foo\"), status: status }  }",
            false,
        );
    let def2 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/get2",
        "\"worker2\"",
        "${ {body: golem:it/api.{get-cart-contents}(\"foo\")} }",
        true,
    );

    let def3 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/get1",
        "\"worker2\"",
        "${ {body: golem:it/api.{get-cart-contents}(\"foo\")} }",
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

    let deployment = get_api_deployment("test-conflict.com", None, vec![&def1.id.0, &def2.id.0]);
    deployment_service
        .deploy(&deployment, &EmptyAuthCtx::default())
        .await
        .unwrap();

    let deployment = get_api_deployment("test-conflict.com", None, vec![&def3.id.0]);
    let deployment_result = deployment_service
        .deploy(&deployment, &EmptyAuthCtx::default())
        .await;
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
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
            false,
        );
    let def1v1_upd = get_api_definition(
            &def1v1.id.0,
            "0.0.1",
            "/api/get1/1",
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
            false,
        );
    let def1v2 = get_api_definition(
            &def1v1.id.0,
            "0.0.2",
            "/api/get1/2",
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
            true,
        );

    let def1v2_upd = get_api_definition(
            &def1v1.id.0,
            "0.0.2",
            "/api/get1/22",
            "${let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; \"shopping-cart-${res}\"}",
            "${ let not_found: u64 = 401; let success: u64 = 200; let result = golem:it/api.{get-cart-contents}(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
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

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all_versions(
            &def1v1.id,
            &DefaultNamespace::default(),
            &EmptyAuthCtx::default(),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1v1.clone(), def1v2.clone()]
    ));

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

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all_versions(
            &def1v1.id,
            &DefaultNamespace::default(),
            &EmptyAuthCtx::default(),
        )
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1v1.clone(), def1v2_upd.clone()]
    ));

    assert!(
        definition_service
            .delete(
                &def1v1.id,
                &def1v1.version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .is_ok(),
        "Failed to delete definition"
    );
    assert!(
        definition_service
            .delete(
                &def1v2.id,
                &def1v2.version,
                &DefaultNamespace::default(),
                &EmptyAuthCtx::default(),
            )
            .await
            .is_ok(),
        "Failed to delete definition"
    );

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

async fn test_delete_non_existing(
    definition_service: Arc<
        dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace, RouteValidationError>
            + Sync
            + Send,
    >,
) {
    let delete_result = definition_service
        .delete(
            &ApiDefinitionId("non-existing".to_string()),
            &ApiVersion("0.0.1".to_string()),
            &DefaultNamespace::default(),
            &EmptyAuthCtx::default(),
        )
        .await;

    assert!(delete_result.is_err(), "definition should not exist");
}

fn get_api_deployment(
    host: &str,
    subdomain: Option<&str>,
    definitions: Vec<&str>,
) -> ApiDeploymentRequest<DefaultNamespace> {
    let api_definition_keys: Vec<ApiDefinitionIdWithVersion> = definitions
        .into_iter()
        .map(|id| ApiDefinitionIdWithVersion {
            id: ApiDefinitionId(id.to_string()),
            version: ApiVersion("0.0.1".to_string()),
        })
        .collect();

    ApiDeploymentRequest {
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
    response_mapping: &str,
    draft: bool,
) -> HttpApiDefinitionRequest {
    let yaml_string = format!(
        r#"
          id: {}
          version: {}
          draft: {}
          routes:
          - method: Get
            path: {}
            binding:
              componentId:
                componentId: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
                version: 0
              workerName: '{}'
              response: '{}'
        "#,
        id, version, draft, path_pattern, worker_id, response_mapping
    );

    serde_yaml::from_str(yaml_string.as_str()).unwrap()
}

fn contains_definitions(
    result: Vec<HttpApiDefinition>,
    expected: Vec<HttpApiDefinitionRequest>,
) -> bool {
    let requests: Vec<HttpApiDefinitionRequest> =
        result.into_iter().map(|x| x.into()).collect::<Vec<_>>();

    for value in expected.into_iter() {
        if !requests.contains(&value) {
            return false;
        }
    }

    true
}
