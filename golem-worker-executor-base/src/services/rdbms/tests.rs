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

use crate::services::rdbms::types::{DbRow, DbValue, DbValuePrimitive};
use crate::services::rdbms::RdbmsServiceDefault;
use crate::services::rdbms::{RdbmsPoolKey, RdbmsService};
use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, WorkerId};
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdbs;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdbs;
use test_r::{test, test_dep};
use uuid::Uuid;

#[test_dep]
async fn postgres() -> DockerPostgresRdbs {
    DockerPostgresRdbs::new(3, 5444, true, false).await
}

#[test_dep]
async fn mysql() -> DockerMysqlRdbs {
    DockerMysqlRdbs::new(3, 3307, true, false).await
}

#[test]
async fn postgres_test(postgres: &DockerPostgresRdbs) {
    let rdbms_service = RdbmsServiceDefault::default();

    for rdb in postgres.rdbs.iter() {
        let address = rdb.postgres_info().host_connection_string();
        println!("address: {}", address);
        let worker_id = WorkerId {
            component_id: ComponentId::new_v4(),
            worker_name: "test".to_string(),
        };

        let connection = rdbms_service.postgres().create(&worker_id, &address).await;

        assert!(connection.is_ok());

        let pool_key = connection.unwrap();
        println!("pool_key: {}", pool_key);
        let result = rdbms_service
            .postgres()
            .execute(&worker_id, &pool_key, "SELECT 1", vec![])
            .await;

        assert!(result.is_ok());
    }

    let address = postgres.rdbs[0].postgres_info().host_connection_string();
    println!("address: {}", address);

    let worker_id = WorkerId {
        component_id: ComponentId::new_v4(),
        worker_name: "test".to_string(),
    };

    let connection = rdbms_service.postgres().create(&worker_id, &address).await;

    assert!(connection.is_ok());

    let pool_key = connection.unwrap();

    let result = rdbms_service
        .postgres()
        .execute(&worker_id, &pool_key, "SELECT 1", vec![])
        .await;

    assert!(result.is_ok());

    let connection = rdbms_service.postgres().create(&worker_id, &address).await;

    assert!(connection.is_ok());

    let exists = rdbms_service.postgres().exists(&worker_id, &pool_key);

    assert!(exists);

    let result = rdbms_service
        .postgres()
        .query(&worker_id, &pool_key, "SELECT 1", vec![])
        .await;

    assert!(result.is_ok());

    let columns = result.unwrap().get_columns().await.unwrap();
    // println!("columns: {columns:?}");
    assert!(!columns.is_empty());

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

    let result = rdbms_service
        .postgres()
        .execute(&worker_id, &pool_key, create_table_statement, vec![])
        .await;

    assert!(result.is_ok());

    for _ in 0..100 {
        let params: Vec<DbValue> = vec![
            DbValue::Primitive(DbValuePrimitive::Uuid(Uuid::new_v4())),
            DbValue::Primitive(DbValuePrimitive::Text("default".to_string())),
            DbValue::Primitive(DbValuePrimitive::Text(format!("name-{}", Uuid::new_v4()))),
        ];

        let result = rdbms_service
            .postgres()
            .execute(&worker_id, &pool_key, insert_statement, params)
            .await;

        assert!(result.is_ok_and(|v| v == 1));
    }

    let result = rdbms_service
        .postgres()
        .query(&worker_id, &pool_key, "SELECT * from components", vec![])
        .await;

    assert!(result.is_ok());

    let result = result.unwrap();

    let columns = result.get_columns().await.unwrap();
    // println!("columns: {columns:?}");
    assert!(!columns.is_empty());

    let mut rows: Vec<DbRow> = vec![];

    while let Some(vs) = result.get_next().await.unwrap() {
        rows.extend(vs);
    }
    // println!("rows: {rows:?}");
    assert!(rows.len() >= 100);
    println!("rows: {}", rows.len());

    let removed = rdbms_service.postgres().remove(&worker_id, &pool_key);

    assert!(removed);

    let exists = rdbms_service.postgres().exists(&worker_id, &pool_key);

    assert!(!exists);
}

#[test]
async fn mysql_test(mysql: &DockerMysqlRdbs) {
    let rdbms_service = RdbmsServiceDefault::default();

    for rdb in mysql.rdbs.iter() {
        let address = rdb.mysql_info().host_connection_string();
        println!("address: {}", address);
        let worker_id = WorkerId {
            component_id: ComponentId::new_v4(),
            worker_name: "test".to_string(),
        };
        let connection = rdbms_service.mysql().create(&worker_id, &address).await;

        assert!(connection.is_ok());

        let pool_key = connection.unwrap();
        println!("pool_key: {}", pool_key);
        let result = rdbms_service
            .mysql()
            .execute(&worker_id, &pool_key, "SELECT 1", vec![])
            .await;

        assert!(result.is_ok());
    }

    let address = mysql.rdbs[0].mysql_info().host_connection_string();
    println!("address: {}", address);

    let worker_id = WorkerId {
        component_id: ComponentId::new_v4(),
        worker_name: "test".to_string(),
    };

    let connection = rdbms_service.mysql().create(&worker_id, &address).await;

    assert!(connection.is_ok());
    let pool_key = connection.unwrap();

    let result = rdbms_service
        .mysql()
        .execute(&worker_id, &pool_key, "SELECT 1", vec![])
        .await;

    assert!(result.is_ok());

    let connection = rdbms_service.mysql().create(&worker_id, &address).await;

    assert!(connection.is_ok());

    let exists = rdbms_service.mysql().exists(&worker_id, &pool_key);

    assert!(exists);

    let result = rdbms_service
        .mysql()
        .query(&worker_id, &pool_key, "SELECT 1", vec![])
        .await;

    assert!(result.is_ok());

    let columns = result.unwrap().get_columns().await.unwrap();
    // println!("columns: {columns:?}");
    assert!(!columns.is_empty());

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

    let result = rdbms_service
        .mysql()
        .execute(&worker_id, &pool_key, create_table_statement, vec![])
        .await;

    assert!(result.is_ok());

    for _ in 0..100 {
        let params: Vec<DbValue> = vec![
            DbValue::Primitive(DbValuePrimitive::Text(Uuid::new_v4().to_string())),
            DbValue::Primitive(DbValuePrimitive::Text("default".to_string())),
            DbValue::Primitive(DbValuePrimitive::Text(format!("name-{}", Uuid::new_v4()))),
        ];

        let result = rdbms_service
            .mysql()
            .execute(&worker_id, &pool_key, insert_statement, params)
            .await;

        assert!(result.is_ok_and(|v| v == 1));
    }

    let result = rdbms_service
        .mysql()
        .query(&worker_id, &pool_key, "SELECT * from components", vec![])
        .await;

    assert!(result.is_ok());

    let result = result.unwrap();

    let columns = result.get_columns().await.unwrap();
    // println!("columns: {columns:?}");
    assert!(!columns.is_empty());

    let mut rows: Vec<DbRow> = vec![];

    while let Some(vs) = result.get_next().await.unwrap() {
        rows.extend(vs);
    }

    // println!("rows: {rows:?}");
    assert!(rows.len() >= 100);
    println!("rows: {}", rows.len());

    let removed = rdbms_service.mysql().remove(&worker_id, &pool_key);

    assert!(removed);

    let exists = rdbms_service.mysql().exists(&worker_id, &pool_key);

    assert!(!exists);
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
