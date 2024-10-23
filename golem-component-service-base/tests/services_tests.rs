use test_r::test;

use golem_common::config::{DbPostgresConfig, DbSqliteConfig};
use golem_service_base::auth::DefaultNamespace;
use golem_service_base::config::ComponentStoreLocalConfig;
use golem_service_base::db;

use golem_common::model::{ComponentId, ComponentType};
use golem_component_service_base::model::Component;
use golem_component_service_base::repo::component::{ComponentRepo, DbComponentRepo};
use golem_component_service_base::service::component::{
    create_new_component, ComponentService, ComponentServiceDefault,
};
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDisabled,
};
use golem_service_base::model::ComponentName;
use golem_service_base::service::component_object_store;
use rib::WorkerFunctionsInRib;
use std::sync::Arc;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

test_r::enable!();

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

struct SqliteDb {
    db_path: String,
}

impl Default for SqliteDb {
    fn default() -> Self {
        Self {
            db_path: format!("/tmp/golem-component-{}.db", Uuid::new_v4()),
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

    db::postgres_migrate(
        &db_config,
        "../golem-component-service/db/migration/postgres",
    )
    .await
    .unwrap();

    let db_pool = db::create_postgres_pool(&db_config).await.unwrap();

    let component_repo: Arc<dyn ComponentRepo + Sync + Send> =
        Arc::new(DbComponentRepo::new(db_pool.clone().into()));

    test_repo(component_repo.clone()).await;
    test_services(component_repo.clone()).await;
}

#[test]
pub async fn test_with_sqlite_db() {
    let db = SqliteDb::default();
    let db_config = DbSqliteConfig {
        database: db.db_path.clone(),
        max_connections: 10,
    };

    db::sqlite_migrate(&db_config, "../golem-component-service/db/migration/sqlite")
        .await
        .unwrap();

    let db_pool = db::create_sqlite_pool(&db_config).await.unwrap();

    let component_repo: Arc<dyn ComponentRepo + Sync + Send> =
        Arc::new(DbComponentRepo::new(db_pool.clone().into()));

    test_repo(component_repo.clone()).await;
    test_services(component_repo.clone()).await;
}

fn get_component_data(name: &str) -> Vec<u8> {
    let path = format!("../test-components/{}.wasm", name);
    std::fs::read(path).unwrap()
}

async fn test_services(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let object_store: Arc<dyn component_object_store::ComponentObjectStore + Sync + Send> =
        Arc::new(
            component_object_store::FsComponentObjectStore::new(&ComponentStoreLocalConfig {
                root_path: "/tmp/component".to_string(),
                object_prefix: Uuid::new_v4().to_string(),
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

    let component_name1 = ComponentName("shopping-cart".to_string());
    let component_name2 = ComponentName("rust-echo".to_string());

    let component1 = component_service
        .create(
            &ComponentId::new_v4(),
            &component_name1,
            ComponentType::Durable,
            get_component_data("shopping-cart"),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();

    let component2 = component_service
        .create(
            &ComponentId::new_v4(),
            &component_name2,
            ComponentType::Durable,
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
            None,
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
        .get_namespace(&component2.versioned_component_id.component_id)
        .await
        .unwrap();
    assert!(component2_result.is_some());
    assert_eq!(component2_result.unwrap(), DefaultNamespace::default());

    let component1_result = component_service
        .download(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(!component1_result.is_empty());

    let component2_result = component_service
        .download(
            &component2.versioned_component_id.component_id,
            None,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(!component2_result.is_empty());

    let component1_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_some());

    let component1_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            Some(10000000),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_none());

    let component2_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            None,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component2_result.is_some());

    let component1_result = component_service
        .find_id_by_name(&component1.component_name, &DefaultNamespace::default())
        .await
        .unwrap();
    assert_eq!(
        component1_result,
        Some(component1.versioned_component_id.component_id.clone())
    );

    let component2_result = component_service
        .find_id_by_name(&component2.component_name, &DefaultNamespace::default())
        .await
        .unwrap();
    assert_eq!(
        component2_result,
        Some(component2.versioned_component_id.component_id.clone())
    );

    let component1_result = component_service
        .find_by_name(
            Some(component1.component_name.clone()),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        component1_result,
        vec![component1.clone(), component1v2.clone()]
    );

    let component2_result = component_service
        .find_by_name(
            Some(component2.component_name.clone()),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert_eq!(component2_result, vec![component2.clone()]);

    let component_result = component_service
        .find_by_name(None, &DefaultNamespace::default())
        .await
        .unwrap();
    assert_eq!(component_result.len(), 3);

    component_service
        .delete(
            &component1v2.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();

    let component1_result = component_service
        .get(
            &component1.versioned_component_id.component_id,
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_empty());

    let component1_result = component_service
        .get_protected_data(
            &component1v2.versioned_component_id.component_id,
            Some(component1v2.versioned_component_id.version),
            &DefaultNamespace::default(),
        )
        .await
        .unwrap();
    assert!(component1_result.is_none());
}

async fn test_repo(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    test_repo_component_id_unique(component_repo.clone()).await;
    test_repo_component_name_unique_in_namespace(component_repo.clone()).await;
    test_repo_component_delete(component_repo.clone()).await;
    test_repo_component_constraints(component_repo.clone()).await;
}

async fn test_repo_component_id_unique(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let namespace1 = Uuid::new_v4().to_string();
    let namespace2 = Uuid::new_v4().to_string();

    let component_name1 = ComponentName("shopping-cart1".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = create_new_component(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();

    let result1 = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;
    let result2 = component_repo
        .create(&component1.clone().next_version().try_into().unwrap())
        .await;
    let result3 = component_repo
        .create(
            &Component {
                namespace: namespace2.clone(),
                ..component1.clone()
            }
            .try_into()
            .unwrap(),
        )
        .await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert!(result3.is_err());
}

async fn test_repo_component_name_unique_in_namespace(
    component_repo: Arc<dyn ComponentRepo + Sync + Send>,
) {
    let namespace1 = Uuid::new_v4().to_string();
    let namespace2 = Uuid::new_v4().to_string();

    let component_name1 = ComponentName("shopping-cart1".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = create_new_component(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();
    let component2 = create_new_component(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace2,
    )
    .unwrap();

    let result1 = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;
    let result2 = component_repo
        .create(
            &Component {
                namespace: namespace2.clone(),
                ..component1.clone()
            }
            .try_into()
            .unwrap(),
        )
        .await;
    let result3 = component_repo
        .create(&component2.clone().try_into().unwrap())
        .await;

    assert!(result1.is_ok());
    assert!(result2.is_err());
    assert!(result3.is_ok());
}

async fn test_repo_component_delete(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let namespace1 = Uuid::new_v4().to_string();

    let component_name1 = ComponentName("shopping-cart1".to_string());
    let data = get_component_data("shopping-cart");

    let component1 = create_new_component(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();

    let result1 = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;

    let result2 = component_repo
        .get(&component1.versioned_component_id.component_id.0)
        .await;

    let result3 = component_repo
        .delete(
            &namespace1,
            &component1.versioned_component_id.component_id.0,
        )
        .await;

    let result4 = component_repo
        .get(&component1.versioned_component_id.component_id.0)
        .await;

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert_eq!(result2.unwrap().len(), 1);
    assert!(result3.is_ok());
    assert!(result4.is_ok());
    assert!(result4.unwrap().is_empty());
}

async fn test_repo_component_constraints(component_repo: Arc<dyn ComponentRepo + Sync + Send>) {
    let namespace1 = Uuid::new_v4().to_string();

    let component_name1 = ComponentName("shopping-cart1".to_string());

    // It has a function golem:it/api.{initialize-cart}(user-id: string)
    let data = get_component_data("shopping-cart");

    let component1 = create_new_component(
        &ComponentId::new_v4(),
        &component_name1,
        ComponentType::Durable,
        &data,
        &namespace1,
    )
    .unwrap();

    let component_constraint_initial = constraint_data::get_shopping_cart_component_constraint1(
        &namespace1,
        &component1.versioned_component_id.component_id,
    );

    let component_constraint_initial_db_record = component_constraint_initial.try_into().unwrap();

    // Create Component
    let component_create_result = component_repo
        .create(&component1.clone().try_into().unwrap())
        .await;

    // Create Constraint
    let component_constraint_create_result = component_repo
        .create_or_update_constraint(&component_constraint_initial_db_record)
        .await;

    // Get constraint
    let result_constraint_get = component_repo
        .get_constraint(&component1.versioned_component_id.component_id)
        .await
        .unwrap();

    let expected_initial_constraint =
        Some(constraint_data::get_shopping_cart_worker_functions_in_rib1());

    let component_constraint_later = constraint_data::get_shopping_cart_component_constraint2(
        &namespace1,
        &component1.versioned_component_id.component_id,
    );

    let component_constraint_later_db_record = component_constraint_later.try_into().unwrap();

    // Update constraint
    let component_constraint_update_result = component_repo
        .create_or_update_constraint(&component_constraint_later_db_record)
        .await;

    // Get updated constraint
    let result_constraint_get_updated = component_repo
        .get_constraint(&component1.versioned_component_id.component_id)
        .await
        .unwrap();

    let expected_updated_constraint = {
        let mut function_calls =
            constraint_data::get_shopping_cart_worker_functions_in_rib2().function_calls;
        function_calls
            .extend(constraint_data::get_shopping_cart_worker_functions_in_rib1().function_calls);
        Some(WorkerFunctionsInRib { function_calls })
    };

    assert!(component_create_result.is_ok());
    assert!(component_constraint_create_result.is_ok());
    assert_eq!(result_constraint_get, expected_initial_constraint);
    assert!(component_constraint_update_result.is_ok());
    assert_eq!(result_constraint_get_updated, expected_updated_constraint);
}

mod constraint_data {
    use golem_common::model::ComponentId;
    use golem_component_service_base::model::ComponentConstraint;
    use golem_wasm_ast::analysis::analysed_type::{f32, list, record, str, u32};
    use golem_wasm_ast::analysis::NameTypePair;
    use rib::{RegistryKey, WorkerFunctionInRibMetadata, WorkerFunctionsInRib};

    pub(crate) fn get_shopping_cart_worker_functions_in_rib1() -> WorkerFunctionsInRib {
        WorkerFunctionsInRib {
            function_calls: vec![WorkerFunctionInRibMetadata {
                function_key: RegistryKey::FunctionNameWithInterface {
                    interface_name: "golem:it/api".to_string(),
                    function_name: "initialize-cart".to_string(),
                },
                parameter_types: vec![str()],
                return_types: vec![],
            }],
        }
    }

    pub(crate) fn get_shopping_cart_worker_functions_in_rib2() -> WorkerFunctionsInRib {
        WorkerFunctionsInRib {
            function_calls: vec![WorkerFunctionInRibMetadata {
                function_key: RegistryKey::FunctionNameWithInterface {
                    interface_name: "golem:it/api".to_string(),
                    function_name: "get-cart-contents".to_string(),
                },
                parameter_types: vec![],
                return_types: vec![list(record(vec![
                    NameTypePair {
                        name: "product_id".to_string(),
                        typ: str(),
                    },
                    NameTypePair {
                        name: "name".to_string(),
                        typ: str(),
                    },
                    NameTypePair {
                        name: "price".to_string(),
                        typ: f32(),
                    },
                    NameTypePair {
                        name: "quantity".to_string(),
                        typ: u32(),
                    },
                ]))],
            }],
        }
    }

    pub(crate) fn get_shopping_cart_component_constraint1(
        namespace: &str,
        component_id: &ComponentId,
    ) -> ComponentConstraint<String> {
        ComponentConstraint {
            namespace: namespace.to_string(),
            component_id: component_id.clone(),
            constraints: get_shopping_cart_worker_functions_in_rib1(),
        }
    }

    pub(crate) fn get_shopping_cart_component_constraint2(
        namespace: &str,
        component_id: &ComponentId,
    ) -> ComponentConstraint<String> {
        ComponentConstraint {
            namespace: namespace.to_string(),
            component_id: component_id.clone(),
            constraints: get_shopping_cart_worker_functions_in_rib2(),
        }
    }
}
