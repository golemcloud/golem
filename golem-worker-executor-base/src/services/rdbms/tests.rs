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
    DbColumn, DbColumnType, DbColumnTypePrimitive, DbRow, DbValue, DbValuePrimitive,
};
use crate::services::rdbms::{Rdbms, RdbmsServiceDefault, RdbmsType};
use crate::services::rdbms::{RdbmsPoolKey, RdbmsService};
use golem_common::model::{ComponentId, WorkerId};
use golem_test_framework::components::rdb::docker_mysql::{DockerMysqlRdb, DockerMysqlRdbs};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdbs;
use golem_test_framework::components::rdb::RdbConnectionString;
use std::ops::Deref;
use std::sync::Arc;
use test_r::{test, test_dep};
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
// async fn make(r: &mut DynamicTestRegistration, suffix: &'static str, query: &'static str, expected: u64) {
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
//             // assert!(connection.is_ok());
//             // assert_eq!(1, expected);
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
        postgres.rdbs[0].deref(),
        rdbms_service.postgres(),
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
    let rdb = postgres.rdbs[1].clone();
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
        rdb.deref(),
        rdbms.clone(),
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
            rdb.deref(),
            rdbms.clone(),
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
        rdb.deref(),
        rdbms.clone(),
        "SELECT component_id, namespace, name from components",
        vec![],
        Some(expected_columns),
        None,
    )
    .await;
}

#[test]
async fn mysql_execute_test_select1(mysql: &DockerMysqlRdbs, rdbms_service: &RdbmsServiceDefault) {
    rdbms_execute_test(
        mysql.rdbs[0].deref(),
        rdbms_service.mysql(),
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
    let rdb = mysql.rdbs[1].clone();
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
        rdb.deref(),
        rdbms.clone(),
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
            rdb.deref(),
            rdbms.clone(),
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
        rdb.deref(),
        rdbms.clone(),
        "SELECT component_id, namespace, name from components",
        vec![],
        Some(expected_columns),
        None,
    )
    .await;
}

async fn rdbms_execute_test<T: RdbmsType>(
    rdb: &impl RdbConnectionString,
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    query: &'static str,
    params: Vec<DbValue>,
    expected: Option<u64>,
) {
    let address = rdb.host_connection_string();
    let worker_id = WorkerId {
        component_id: ComponentId::new_v4(),
        worker_name: "test".to_string(),
    };
    let connection = rdbms.create(&worker_id, &address).await;
    assert!(connection.is_ok());
    let pool_key = connection.unwrap();
    // println!("pool_key: {}", pool_key);
    let result = rdbms.execute(&worker_id, &pool_key, query, params).await;
    assert!(result.is_ok());
    if let Some(expected) = expected {
        assert!(result.unwrap() == expected);
    }
    let _ = rdbms.remove(&worker_id, &pool_key);
}

async fn rdbms_query_test<T: RdbmsType>(
    rdb: &impl RdbConnectionString,
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    query: &'static str,
    params: Vec<DbValue>,
    expected_columns: Option<Vec<DbColumn>>,
    expected_rows: Option<Vec<DbRow>>,
) {
    let address = rdb.host_connection_string();
    let worker_id = WorkerId {
        component_id: ComponentId::new_v4(),
        worker_name: "test".to_string(),
    };
    let connection = rdbms.create(&worker_id, &address).await;
    assert!(connection.is_ok());
    let pool_key = connection.unwrap();
    // println!("pool_key: {}", pool_key);
    let result = rdbms.query(&worker_id, &pool_key, query, params).await;
    assert!(result.is_ok());
    let result = result.unwrap();
    let columns = result.get_columns().await.unwrap();

    if let Some(expected) = expected_columns {
        assert!(columns == expected);
    }

    let mut rows: Vec<DbRow> = vec![];

    while let Some(vs) = result.get_next().await.unwrap() {
        rows.extend(vs);
    }

    if let Some(expected) = expected_rows {
        assert!(rows == expected);
    }

    let _ = rdbms.remove(&worker_id, &pool_key);
}

#[test]
fn test_rdbms_pool_key_masked_address() {
    let key = RdbmsPoolKey::new("mysql://user:password@localhost:3306".to_string());
    assert_eq!(
        key.masked_address(),
        "mysql://user:*****@localhost:3306".to_string()
    );
    let key = RdbmsPoolKey::new("mysql://user@localhost:3306".to_string());
    assert_eq!(
        key.masked_address(),
        "mysql://user@localhost:3306".to_string()
    );
    let key = RdbmsPoolKey::new("mysql://localhost:3306".to_string());
    assert_eq!(key.masked_address(), "mysql://localhost:3306".to_string());
    let key =
        RdbmsPoolKey::new("postgres://user:password@localhost:5432?abc=xyz&def=xyz".to_string());
    assert_eq!(
        key.masked_address(),
        "postgres://user:*****@localhost:5432?abc=xyz&def=xyz".to_string()
    );
}
