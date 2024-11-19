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

use crate::services::rdbms::types::{
    DbColumn, DbColumnType, DbColumnTypePrimitive, DbRow, DbValue, DbValuePrimitive, Error,
};
use crate::services::rdbms::{Rdbms, RdbmsServiceDefault, RdbmsType};
use crate::services::rdbms::{RdbmsPoolKey, RdbmsService};
use assert2::check;
use golem_common::model::{ComponentId, WorkerId};
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdbs;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdbs;
use golem_test_framework::components::rdb::{RdbConnection, RdbsConnections};
use std::collections::HashMap;
use std::sync::Arc;
use test_r::{test, test_dep};
use tokio::task::JoinSet;
use uuid::Uuid;

#[test_dep]
async fn postgres() -> DockerPostgresRdbs {
    DockerPostgresRdbs::new(3, 5434, true, false).await
}

#[test_dep]
async fn mysql() -> DockerMysqlRdbs {
    DockerMysqlRdbs::new(3, 3307, true, false).await
}

#[test_dep]
fn rdbms_service() -> RdbmsServiceDefault {
    RdbmsServiceDefault::default()
}

// #[test_gen]
// async fn generated(r: &mut DynamicTestRegistration) {
//     make(r, "_select1", "SELECT 1", 1).await;
// }
//
// async fn make(r: &mut DynamicTestRegistration, suffix: &str, query: &str, expected: u64) {
//     add_test!(
//         r,
//         format!("postgres_execute_test_{suffix}"),
//         TestType::IntegrationTest,
//         move |postgres: &mut DockerPostgresRdbs, rdbms_service: &mut RdbmsServiceDefault| async {
//
//             //     let address = postgres.rdbs[0].host_connection_string();
//             // println!("address: {}", address);
//             //
//             // let worker_id = WorkerId {
//             //     component_id: ComponentId::new_v4(),
//             //     worker_name: "test".to_string(),
//             // };
//             //
//             // let connection = rdbms_service.postgres().create(&worker_id, &address).await;
//             // check!(connection.is_ok());
//             // check!(1, expected);
//             // postgres_execute_test((postgres, rdbms_service, query.to_string(), expected)).await.as_result()
//         }
//     );
// }

#[test]
async fn postgres_execute_test_select1(
    postgres: &DockerPostgresRdbs,
    rdbms_service: &RdbmsServiceDefault,
) {
    rdbms_execute_test(
        rdbms_service.postgres(),
        &postgres.rdbs[0].host_connection_string(),
        "SELECT 1",
        vec![],
        Some(1),
    )
    .await
}

#[test]
async fn postgres_par_test(postgres: &DockerPostgresRdbs, rdbms_service: &RdbmsServiceDefault) {
    rdbms_par_test(
        rdbms_service.postgres(),
        postgres.host_connection_strings(),
        3,
        "SELECT 1",
        vec![],
        Some(1),
    )
    .await
}

#[test]
async fn postgres_execute_test_create_insert_select(
    postgres: &DockerPostgresRdbs,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.rdbs[1].host_connection_string();
    let rdbms = rdbms_service.postgres();
    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS components
            (
                component_id        uuid    NOT NULL PRIMARY KEY,
                namespace           text    NOT NULL,
                name                text    NOT NULL
            );
        "#;

    let insert_statement = r#"
            INSERT INTO components
            (component_id, namespace, name)
            VALUES
            ($1, $2, $3)
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_table_statement,
        vec![],
        None,
    )
    .await;

    let count = 100;

    for _ in 0..count {
        let params: Vec<DbValue> = vec![
            DbValue::Primitive(DbValuePrimitive::Uuid(Uuid::new_v4())),
            DbValue::Primitive(DbValuePrimitive::Text("default".to_string())),
            DbValue::Primitive(DbValuePrimitive::Text(format!("name-{}", Uuid::new_v4()))),
        ];

        rdbms_execute_test(
            rdbms.clone(),
            &db_address,
            insert_statement,
            params,
            Some(1),
        )
        .await;
    }

    let expected_columns = vec![
        DbColumn {
            name: "component_id".to_string(),
            ordinal: 0,
            db_type: DbColumnType::Primitive(DbColumnTypePrimitive::Uuid),
            db_type_name: "UUID".to_string(),
        },
        DbColumn {
            name: "namespace".to_string(),
            ordinal: 1,
            db_type: DbColumnType::Primitive(DbColumnTypePrimitive::Text),
            db_type_name: "TEXT".to_string(),
        },
        DbColumn {
            name: "name".to_string(),
            ordinal: 2,
            db_type: DbColumnType::Primitive(DbColumnTypePrimitive::Text),
            db_type_name: "TEXT".to_string(),
        },
    ];

    rdbms_query_test(
        rdbms.clone(),
        &db_address,
        "SELECT component_id, namespace, name from components",
        vec![],
        Some(expected_columns),
        None,
    )
    .await;
}

#[test]
async fn mysql_par_test(mysql: &DockerMysqlRdbs, rdbms_service: &RdbmsServiceDefault) {
    rdbms_par_test(
        rdbms_service.mysql(),
        mysql.host_connection_strings(),
        3,
        "SELECT 1",
        vec![],
        Some(0),
    )
    .await
}

#[test]
async fn mysql_execute_test_select1(mysql: &DockerMysqlRdbs, rdbms_service: &RdbmsServiceDefault) {
    rdbms_execute_test(
        rdbms_service.mysql(),
        &mysql.rdbs[0].host_connection_string(),
        "SELECT 1",
        vec![],
        Some(0),
    )
    .await
}

#[test]
async fn mysql_execute_test_create_insert_select(
    mysql: &DockerMysqlRdbs,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = mysql.rdbs[1].clone().host_connection_string();
    let rdbms = rdbms_service.mysql();
    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS components
            (
                component_id        varchar(255)    NOT NULL,
                namespace           varchar(255)    NOT NULL,
                name                varchar(255)    NOT NULL,
                PRIMARY KEY (component_id)
            );
        "#;

    let insert_statement = r#"
            INSERT INTO components
            (component_id, namespace, name)
            VALUES
            (?, ?, ?)
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_table_statement,
        vec![],
        None,
    )
    .await;

    let count = 100;

    for _ in 0..count {
        let params: Vec<DbValue> = vec![
            DbValue::Primitive(DbValuePrimitive::Text(Uuid::new_v4().to_string())),
            DbValue::Primitive(DbValuePrimitive::Text("default".to_string())),
            DbValue::Primitive(DbValuePrimitive::Text(format!("name-{}", Uuid::new_v4()))),
        ];

        rdbms_execute_test(
            rdbms.clone(),
            &db_address,
            insert_statement,
            params,
            Some(1),
        )
        .await;
    }

    let expected_columns = vec![
        DbColumn {
            name: "component_id".to_string(),
            ordinal: 0,
            db_type: DbColumnType::Primitive(DbColumnTypePrimitive::Text),
            db_type_name: "VARCHAR".to_string(),
        },
        DbColumn {
            name: "namespace".to_string(),
            ordinal: 1,
            db_type: DbColumnType::Primitive(DbColumnTypePrimitive::Text),
            db_type_name: "VARCHAR".to_string(),
        },
        DbColumn {
            name: "name".to_string(),
            ordinal: 2,
            db_type: DbColumnType::Primitive(DbColumnTypePrimitive::Text),
            db_type_name: "VARCHAR".to_string(),
        },
    ];

    rdbms_query_test(
        rdbms.clone(),
        &db_address,
        "SELECT component_id, namespace, name from components",
        vec![],
        Some(expected_columns),
        None,
    )
    .await;
}

async fn rdbms_execute_test<T: RdbmsType>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    query: &str,
    params: Vec<DbValue>,
    expected: Option<u64>,
) {
    let worker_id = new_worker_id();
    let connection = rdbms.create(&worker_id, db_address).await;
    check!(connection.is_ok());
    let pool_key = connection.unwrap();
    // println!("pool_key: {}", pool_key);
    let result = rdbms.execute(&worker_id, &pool_key, query, params).await;
    check!(result.is_ok());
    if let Some(expected) = expected {
        check!(result.unwrap() == expected);
    }
    let _ = rdbms.remove(&worker_id, &pool_key);
    let exists = rdbms.exists(&worker_id, &pool_key);
    check!(!exists);
}

async fn rdbms_query_test<T: RdbmsType>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    query: &str,
    params: Vec<DbValue>,
    expected_columns: Option<Vec<DbColumn>>,
    expected_rows: Option<Vec<DbRow>>,
) {
    let worker_id = new_worker_id();
    let connection = rdbms.create(&worker_id, db_address).await;
    check!(connection.is_ok());
    let pool_key = connection.unwrap();

    let result = rdbms.query(&worker_id, &pool_key, query, params).await;
    check!(result.is_ok());

    let result = result.unwrap();
    let columns = result.get_columns().await.unwrap();

    if let Some(expected) = expected_columns {
        check!(columns == expected);
    }

    let mut rows: Vec<DbRow> = vec![];

    while let Some(vs) = result.get_next().await.unwrap() {
        rows.extend(vs);
    }

    if let Some(expected) = expected_rows {
        check!(rows == expected);
    }

    let _ = rdbms.remove(&worker_id, &pool_key);

    let exists = rdbms.exists(&worker_id, &pool_key);
    check!(!exists);
}

async fn rdbms_par_test<T: RdbmsType + 'static>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_addresses: Vec<String>,
    count: u8,
    query: &'static str,
    params: Vec<DbValue>,
    expected: Option<u64>,
) {
    let mut fibers = JoinSet::new();
    for db_address in db_addresses {
        for _ in 0..count {
            let rdbms_clone = rdbms.clone();
            let db_address_clone = db_address.clone();
            let params_clone = params.clone();
            let _ = fibers.spawn(async move {
                let worker_id = new_worker_id();

                let connection = rdbms_clone.create(&worker_id, &db_address_clone).await;

                let pool_key = connection.unwrap();

                let result: Result<u64, Error> = rdbms_clone
                    .execute(&worker_id, &pool_key, query, params_clone)
                    .await;

                (worker_id, pool_key, result)
            });
        }
    }

    let mut workers_results: HashMap<WorkerId, Result<u64, Error>> = HashMap::new();
    let mut workers_pools: HashMap<WorkerId, RdbmsPoolKey> = HashMap::new();

    while let Some(res) = fibers.join_next().await {
        let (worker_id, pool_key, result_execute) = res.unwrap();
        workers_results.insert(worker_id.clone(), result_execute);
        workers_pools.insert(worker_id.clone(), pool_key);
    }

    let rdbms_status = rdbms.status();

    for (worker_id, pool_key) in workers_pools.clone() {
        let _ = rdbms.remove(&worker_id, &pool_key);
    }

    for (worker_id, pool_key) in workers_pools {
        let worker_ids = rdbms_status.pools.get(&pool_key);

        check!(
            worker_ids.is_some_and(|ids| ids.contains(&worker_id)),
            "worker {worker_id} not found in pool {pool_key}"
        );
    }

    for (worker_id, result) in workers_results {
        check!(result.is_ok(), "result for worker {worker_id} is error");

        if let Some(expected) = expected {
            check!(result.unwrap() == expected, "result for worker {worker_id}");
        }
    }
}

#[test]
async fn postgres_schema_test(postgres: &DockerPostgresRdbs, rdbms_service: &RdbmsServiceDefault) {
    let rdbms = rdbms_service.postgres();
    let schema = "test1";
    let db_address = postgres.rdbs[2].host_connection_string();
    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        format!("CREATE SCHEMA IF NOT EXISTS {};", schema).as_str(),
        vec![],
        None,
    )
    .await;

    let create_table_statement = format!(
        r#"
            CREATE TABLE IF NOT EXISTS {schema}.components
            (
                component_id        varchar(255)    NOT NULL,
                namespace           varchar(255)    NOT NULL,
                name                varchar(255)    NOT NULL,
                PRIMARY KEY (component_id)
            );
        "#
    );
    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_table_statement.as_str(),
        vec![],
        None,
    )
    .await;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        format!("SELECT component_id, namespace, name from {schema}.components").as_str(),
        vec![],
        Some(0),
    )
    .await
}

#[test]
fn test_rdbms_pool_key_masked_address() {
    let key = RdbmsPoolKey::new("mysql://user:password@localhost:3306".to_string());
    check!(key.masked_address() == "mysql://user:*****@localhost:3306".to_string());
    let key = RdbmsPoolKey::new("mysql://user@localhost:3306".to_string());
    check!(key.masked_address() == "mysql://user@localhost:3306".to_string());
    let key = RdbmsPoolKey::new("mysql://localhost:3306".to_string());
    check!(key.masked_address() == "mysql://localhost:3306".to_string());
    let key =
        RdbmsPoolKey::new("postgres://user:password@localhost:5432?abc=xyz&def=xyz".to_string());
    check!(
        key.masked_address() == "postgres://user:*****@localhost:5432?abc=xyz&def=xyz".to_string()
    );
}

fn new_worker_id() -> WorkerId {
    WorkerId {
        component_id: ComponentId::new_v4(),
        worker_name: "test".to_string(),
    }
}
