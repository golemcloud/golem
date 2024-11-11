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

#[cfg(test)]
mod tests {
    use crate::services::rdbms::types::{DbRow, DbValue, DbValuePrimitive};
    use crate::services::rdbms::RdbmsService;
    use crate::services::rdbms::RdbmsServiceDefault;
    use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, WorkerId};
    use test_r::test;
    use uuid::Uuid;

    #[test]
    async fn postgres_test() {
        let rdbms_service = RdbmsServiceDefault::default();

        let address = "postgresql://postgres:postgres@localhost:5444/postgres";

        let worker_id = OwnedWorkerId::new(
            &AccountId::generate(),
            &WorkerId {
                component_id: ComponentId::new_v4(),
                worker_name: "test".to_string(),
            },
        );

        let connection = rdbms_service.postgres().create(&worker_id, address).await;

        assert!(connection.is_ok());

        let result = rdbms_service
            .postgres()
            .execute(&worker_id, address, "SELECT 1", vec![])
            .await;

        assert!(result.is_ok());

        let connection = rdbms_service.postgres().create(&worker_id, address).await;

        assert!(connection.is_ok());

        let exists = rdbms_service.postgres().exists(&worker_id, address);

        assert!(exists);

        let result = rdbms_service
            .postgres()
            .query(&worker_id, address, "SELECT 1", vec![])
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
            .execute(&worker_id, address, create_table_statement, vec![])
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
                .execute(&worker_id, address, insert_statement, params)
                .await;

            assert!(result.is_ok_and(|v| v == 1));
        }

        let result = rdbms_service
            .postgres()
            .query(&worker_id, address, "SELECT * from components", vec![])
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

        let removed = rdbms_service.postgres().remove(&worker_id, address);

        assert!(removed);

        let exists = rdbms_service.postgres().exists(&worker_id, address);

        assert!(!exists);
    }

    #[test]
    async fn mysql_test() {
        let rdbms_service = RdbmsServiceDefault::default();

        let address = "mysql://root:mysql@localhost:3307/mysql";

        let worker_id = OwnedWorkerId::new(
            &AccountId::generate(),
            &WorkerId {
                component_id: ComponentId::new_v4(),
                worker_name: "test".to_string(),
            },
        );

        let connection = rdbms_service.mysql().create(&worker_id, address).await;

        assert!(connection.is_ok());

        let result = rdbms_service
            .mysql()
            .execute(&worker_id, address, "SELECT 1", vec![])
            .await;

        assert!(result.is_ok());

        let connection = rdbms_service.mysql().create(&worker_id, address).await;

        assert!(connection.is_ok());

        let exists = rdbms_service.mysql().exists(&worker_id, address);

        assert!(exists);

        let result = rdbms_service
            .mysql()
            .query(&worker_id, address, "SELECT 1", vec![])
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
            .execute(&worker_id, address, create_table_statement, vec![])
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
                .execute(&worker_id, address, insert_statement, params)
                .await;

            assert!(result.is_ok_and(|v| v == 1));
        }

        let result = rdbms_service
            .mysql()
            .query(&worker_id, address, "SELECT * from components", vec![])
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

        let removed = rdbms_service.mysql().remove(&worker_id, address);

        assert!(removed);

        let exists = rdbms_service.mysql().exists(&worker_id, address);

        assert!(!exists);
    }
}
