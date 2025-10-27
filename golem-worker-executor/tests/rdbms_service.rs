// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use assert2::check;
use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use golem_common::model::{ComponentId, TransactionId, WorkerId};
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdb;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_executor::services::golem_config::{RdbmsConfig, RdbmsPoolConfig};
use golem_worker_executor::services::rdbms::mysql::{types as mysql_types, MysqlType};
use golem_worker_executor::services::rdbms::postgres::{types as postgres_types, PostgresType};
use golem_worker_executor::services::rdbms::{DbResult, DbRow, Error, RdbmsTransactionStatus};
use golem_worker_executor::services::rdbms::{Rdbms, RdbmsServiceDefault, RdbmsType};
use golem_worker_executor::services::rdbms::{RdbmsPoolKey, RdbmsService};
use mac_address::MacAddress;
use serde_json::json;
use std::any::{Any, TypeId};
use std::collections::{Bound, HashMap};
use std::fmt::Debug;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::Arc;
use test_r::{test, test_dep};
use tokio::task::JoinSet;
use tracing::{info, Instrument};
use uuid::Uuid;

#[test_dep]
async fn postgres() -> DockerPostgresRdb {
    let unique_network_id = Uuid::new_v4().to_string();
    DockerPostgresRdb::new(&unique_network_id).await
}

#[test_dep]
async fn mysql() -> DockerMysqlRdb {
    let unique_network_id = Uuid::new_v4().to_string();
    DockerMysqlRdb::new(&unique_network_id).await
}

#[test_dep]
fn rdbms_service() -> RdbmsServiceDefault {
    RdbmsServiceDefault::new(RdbmsConfig {
        pool: RdbmsPoolConfig {
            max_connections: 100,
            ..Default::default()
        },
        ..Default::default()
    })
}

#[derive(Clone, Debug)]
enum StatementAction<T: RdbmsType> {
    Execute(ExpectedExecuteResult),
    Query(ExpectedQueryResult<T>),
    QueryStream(ExpectedQueryResult<T>),
}

#[derive(Clone, Debug)]
struct ExpectedQueryResult<T: RdbmsType> {
    expected_columns: Option<Vec<T::DbColumn>>,
    expected_rows: Option<Vec<DbRow<T::DbValue>>>,
}

impl<T: RdbmsType> ExpectedQueryResult<T> {
    fn new(
        expected_columns: Option<Vec<T::DbColumn>>,
        expected_rows: Option<Vec<DbRow<T::DbValue>>>,
    ) -> Self {
        Self {
            expected_rows,
            expected_columns,
        }
    }
}

#[derive(Clone, Debug)]
struct ExpectedExecuteResult {
    expected: Option<u64>,
}

impl ExpectedExecuteResult {
    fn new(expected: Option<u64>) -> Self {
        Self { expected }
    }
}

#[derive(Clone, Debug)]
enum StatementResult<T: RdbmsType + 'static> {
    Execute(u64),
    Query(DbResult<T>),
}

#[derive(Clone, Eq, PartialEq, Debug)]
enum TransactionEnd {
    Commit,
    Rollback,
    // no transaction end action, drop of transaction resource should do rollback
    None,
}

#[derive(Debug, Clone)]
struct StatementTest<T: RdbmsType> {
    pub action: StatementAction<T>,
    pub statement: &'static str,
    pub params: Vec<T::DbValue>,
}

impl<T: RdbmsType> StatementTest<T> {
    fn execute_test(
        statement: &'static str,
        params: Vec<T::DbValue>,
        expected: Option<u64>,
    ) -> Self {
        Self {
            action: StatementAction::Execute(ExpectedExecuteResult::new(expected)),
            statement,
            params,
        }
    }

    fn query_test(
        statement: &'static str,
        params: Vec<T::DbValue>,
        expected_columns: Option<Vec<T::DbColumn>>,
        expected_rows: Option<Vec<DbRow<T::DbValue>>>,
    ) -> Self {
        Self {
            action: StatementAction::Query(ExpectedQueryResult::new(
                expected_columns,
                expected_rows,
            )),
            statement,
            params,
        }
    }

    fn query_stream_test(
        statement: &'static str,
        params: Vec<T::DbValue>,
        expected_columns: Option<Vec<T::DbColumn>>,
        expected_rows: Option<Vec<DbRow<T::DbValue>>>,
    ) -> Self {
        Self {
            action: StatementAction::QueryStream(ExpectedQueryResult::new(
                expected_columns,
                expected_rows,
            )),
            statement,
            params,
        }
    }

    fn with_query_expected(
        &self,
        expected_columns: Option<Vec<T::DbColumn>>,
        expected_rows: Option<Vec<DbRow<T::DbValue>>>,
    ) -> Self {
        Self {
            action: StatementAction::Query(ExpectedQueryResult::new(
                expected_columns,
                expected_rows,
            )),
            ..self.clone()
        }
    }

    fn with_query_stream_expected(
        &self,
        expected_columns: Option<Vec<T::DbColumn>>,
        expected_rows: Option<Vec<DbRow<T::DbValue>>>,
    ) -> Self {
        Self {
            action: StatementAction::QueryStream(ExpectedQueryResult::new(
                expected_columns,
                expected_rows,
            )),
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone)]
struct RdbmsTest<T: RdbmsType> {
    statements: Vec<StatementTest<T>>,
    transaction_end: Option<TransactionEnd>,
}

impl<T: RdbmsType> RdbmsTest<T> {
    fn new(statements: Vec<StatementTest<T>>, transaction_end: Option<TransactionEnd>) -> Self {
        Self {
            statements,
            transaction_end,
        }
    }
}

#[test]
async fn postgres_transaction_tests(
    postgres: &DockerPostgresRdb,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.public_connection_string();
    let rdbms = rdbms_service.postgres();

    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS test_users
            (
                user_id             text    NOT NULL PRIMARY KEY,
                name                text    NOT NULL,
                tags                text[],
                created_on          timestamp DEFAULT NOW()
            );
        "#;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![StatementTest::execute_test(
                create_table_statement,
                vec![],
                None,
            )],
            None,
        ),
    )
    .await;

    let insert_statement = r#"
            INSERT INTO test_users
            (user_id, name, tags)
            VALUES
            ($1, $2, $3)
        "#;

    let count = 60;

    let mut rows: Vec<DbRow<postgres_types::DbValue>> = Vec::with_capacity(count);

    let mut statements: Vec<StatementTest<PostgresType>> = Vec::with_capacity(count);

    for i in 0..count {
        let params: Vec<postgres_types::DbValue> = vec![
            postgres_types::DbValue::Text(format!("{i:03}")),
            postgres_types::DbValue::Text(format!("name-{i:03}")),
            postgres_types::DbValue::Array(vec![
                postgres_types::DbValue::Text(format!("tag-1-{i}")),
                postgres_types::DbValue::Text(format!("tag-2-{i}")),
            ]),
        ];

        statements.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        rows.push(DbRow { values: params });
    }

    let select_statement_test = StatementTest::query_test(
        "SELECT user_id, name, tags FROM test_users ORDER BY user_id ASC",
        vec![],
        None,
        Some(rows.clone()),
    );

    statements.push(select_statement_test.clone());

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(statements, Some(TransactionEnd::Commit)),
    )
    .await;

    let delete_statement_test = StatementTest::execute_test("DELETE FROM test_users", vec![], None);

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![delete_statement_test.clone()],
            Some(TransactionEnd::Rollback),
        ),
    )
    .await;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![select_statement_test.with_query_stream_expected(None, Some(rows.clone()))],
            Some(TransactionEnd::Commit),
        ),
    )
    .await;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(vec![delete_statement_test], Some(TransactionEnd::Commit)),
    )
    .await;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![select_statement_test.with_query_expected(Some(vec![]), Some(vec![]))],
            Some(TransactionEnd::None),
        ),
    )
    .await;
}

#[test]
async fn postgres_create_insert_select_test(
    postgres: &DockerPostgresRdb,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.public_connection_string();
    let rdbms = rdbms_service.postgres();

    let statements = vec![
        r#"
             CREATE TYPE test_enum AS ENUM ('regular', 'special');
        "#,
        r#"
            CREATE TYPE inventory_item AS (
                product_id      uuid,
                name            text,
                supplier_id     INT4,
                price           numeric
            );
        "#,
        r#"
            CREATE DOMAIN posint4 AS INT4 CHECK (VALUE > 0);
        "#,
        r#"
            CREATE TYPE float8range AS RANGE (
                subtype = float8,
                subtype_diff = float8mi
            );
        "#,
        r#"
            CREATE TABLE data_types (
                id VARCHAR(25) PRIMARY KEY,
                enum_col test_enum,
                char_col "char",
                int2_col INT2,
                int4_col INT4,
                int8_col INT8,
                float4_col FLOAT4,
                float8_col FLOAT8,
                numeric_col NUMERIC(10,2),
                boolean_col BOOLEAN,
                text_col TEXT,
                varchar_col VARCHAR(255),
                bpchar_col CHAR(10),
                timestamp_col TIMESTAMP,
                timestamptz_col TIMESTAMP WITH TIME ZONE,
                date_col DATE,
                time_col TIME,
                timetz_col TIME WITH TIME ZONE,
                interval_col INTERVAL,
                bytea_col BYTEA,
                uuid_col UUID,
                json_col JSON,
                jsonb_col JSONB,
                inet_col INET,
                cidr_col CIDR,
                macaddr_col MACADDR,
                bit_col BIT(3),
                varbit_col VARBIT,
                xml_col XML,
                int4range_col INT4RANGE,
                int8range_col INT8RANGE,
                numrange_col NUMRANGE,
                tsrange_col TSRANGE,
                tstzrange_col TSTZRANGE,
                daterange_col DATERANGE,
                money_col MONEY,
                jsonpath_col JSONPATH,
                tsvector_col TSVECTOR,
                tsquery_col TSQUERY,
                inventory_item_col inventory_item,
                posint4_col posint4,
                float8range_col float8range
            );
        "#,
    ];

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            statements
                .into_iter()
                .map(|s| StatementTest::execute_test(s, vec![], Some(0)))
                .collect(),
            Some(TransactionEnd::Commit),
        ),
    )
    .await;

    let insert_statement = r#"
            INSERT INTO data_types
            (
            id,
            enum_col,
            char_col,
            int2_col,
            int4_col,
            int8_col,
            float4_col,
            float8_col,
            numeric_col,
            boolean_col,
            text_col,
            varchar_col,
            bpchar_col,
            timestamp_col,
            timestamptz_col,
            date_col,
            time_col,
            timetz_col,
            interval_col,
            bytea_col,
            uuid_col,
            json_col,
            jsonb_col,
            inet_col,
            cidr_col,
            macaddr_col,
            bit_col,
            varbit_col,
            xml_col,
            int4range_col,
            int8range_col,
            numrange_col,
            tsrange_col,
            tstzrange_col,
            daterange_col,
            money_col,
            jsonpath_col,
            tsvector_col,
            tsquery_col,
            inventory_item_col,
            posint4_col,
            float8range_col
            )
            VALUES
            (
                $1, $2, $3, $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21, $22, $23, $24, $25, $26, $27, $28, $29,
                $30, $31, $32, $33, $34, $35, $36, $37, $38::tsvector, $39::tsquery,
                $40, $41, $42
            );
        "#;

    let count = 4;

    let mut rows: Vec<DbRow<postgres_types::DbValue>> = Vec::with_capacity(count);
    let mut statements: Vec<StatementTest<PostgresType>> = Vec::with_capacity(count);
    for i in 0..count {
        let mut params: Vec<postgres_types::DbValue> =
            vec![postgres_types::DbValue::Varchar(format!("{i:03}"))];

        if i % 2 == 0 {
            let tstzbounds = postgres_types::ValuesRange::new(
                Bound::Included(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 3, 2 + i as u32).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                Bound::Excluded(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2024, 1, 1 + i as u32).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
            );
            let tsbounds = postgres_types::ValuesRange::new(
                Bound::Included(chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2022, 2, 2 + i as u32).unwrap(),
                    chrono::NaiveTime::from_hms_opt(16, 50, 30).unwrap(),
                )),
                Bound::Excluded(chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2024, 1, 1 + i as u32).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )),
            );

            params.append(&mut vec![
                postgres_types::DbValue::Enumeration(postgres_types::Enumeration::new(
                    "test_enum".to_string(),
                    "regular".to_string(),
                )),
                postgres_types::DbValue::Character(2),
                postgres_types::DbValue::Int2(1),
                postgres_types::DbValue::Int4(2),
                postgres_types::DbValue::Int8(3),
                postgres_types::DbValue::Float4(4.0),
                postgres_types::DbValue::Float8(5.0),
                postgres_types::DbValue::Numeric(BigDecimal::from(48888)),
                postgres_types::DbValue::Boolean(true),
                postgres_types::DbValue::Text("text".to_string()),
                postgres_types::DbValue::Varchar("varchar".to_string()),
                postgres_types::DbValue::Bpchar("0123456789".to_string()),
                postgres_types::DbValue::Timestamp(chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )),
                postgres_types::DbValue::Timestamptz(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                postgres_types::DbValue::Date(chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap()),
                postgres_types::DbValue::Time(chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap()),
                postgres_types::DbValue::Timetz(postgres_types::TimeTz::new(
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    chrono::FixedOffset::west_opt(3 * 60 * 60).unwrap(),
                )),
                postgres_types::DbValue::Interval(postgres_types::Interval::new(10, 20, 30)),
                postgres_types::DbValue::Bytea("bytea".as_bytes().to_vec()),
                postgres_types::DbValue::Uuid(Uuid::new_v4()),
                postgres_types::DbValue::Json(
                    json!(
                           {
                              "id": i
                           }
                    )
                    .to_string(),
                ),
                postgres_types::DbValue::Jsonb(
                    json!(
                           {
                              "index": i
                           }
                    )
                    .to_string(),
                ),
                postgres_types::DbValue::Inet(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
                postgres_types::DbValue::Cidr(IpAddr::V6(Ipv6Addr::new(
                    0, 0, 0, 0, 0, 0xffff, 0xc00a, 0x2ff,
                ))),
                postgres_types::DbValue::Macaddr(MacAddress::new([0, 1, 2, 3, 4, i as u8])),
                postgres_types::DbValue::Bit(BitVec::from_iter(vec![true, false, true])),
                postgres_types::DbValue::Varbit(BitVec::from_iter(vec![true, false, false])),
                postgres_types::DbValue::Xml(format!("<foo>{i}</foo>")),
                postgres_types::DbValue::Int4range(postgres_types::ValuesRange::new(
                    Bound::Included(1),
                    Bound::Excluded(4),
                )),
                postgres_types::DbValue::Int8range(postgres_types::ValuesRange::new(
                    Bound::Included(1),
                    Bound::Unbounded,
                )),
                postgres_types::DbValue::Numrange(postgres_types::ValuesRange::new(
                    Bound::Included(BigDecimal::from(11)),
                    Bound::Excluded(BigDecimal::from(221)),
                )),
                postgres_types::DbValue::Tsrange(tsbounds),
                postgres_types::DbValue::Tstzrange(tstzbounds),
                postgres_types::DbValue::Daterange(postgres_types::ValuesRange::new(
                    Bound::Included(
                        chrono::NaiveDate::from_ymd_opt(2023 + i as i32, 2, 3).unwrap(),
                    ),
                    Bound::Unbounded,
                )),
                postgres_types::DbValue::Money(12345),
                postgres_types::DbValue::Jsonpath("$.id".to_string()),
                postgres_types::DbValue::Text(
                    "'a' 'and' 'ate' 'cat' 'fat' 'mat' 'on' 'rat' 'sat'".to_string(),
                ),
                postgres_types::DbValue::Text("'fat' & 'rat' & !'cat'".to_string()),
                postgres_types::DbValue::Composite(postgres_types::Composite::new(
                    "inventory_item".to_string(),
                    vec![
                        postgres_types::DbValue::Uuid(Uuid::new_v4()),
                        postgres_types::DbValue::Text("text".to_string()),
                        postgres_types::DbValue::Int4(i as i32),
                        postgres_types::DbValue::Numeric(BigDecimal::from(111)),
                    ],
                )),
                postgres_types::DbValue::Domain(postgres_types::Domain::new(
                    "posint4".to_string(),
                    postgres_types::DbValue::Int4(1 + i as i32),
                )),
                postgres_types::DbValue::Range(postgres_types::Range::new(
                    "float8range".to_string(),
                    postgres_types::ValuesRange::new(
                        Bound::Included(postgres_types::DbValue::Float8(1.23)),
                        Bound::Excluded(postgres_types::DbValue::Float8(4.55)),
                    ),
                )),
            ]);
        } else {
            for _ in 0..41 {
                params.push(postgres_types::DbValue::Null);
            }
        }

        statements.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        let values = params
            .into_iter()
            .map(|v| match v {
                postgres_types::DbValue::Domain(v) => *v.value,
                _ => v,
            })
            .collect();

        rows.push(DbRow { values });
    }

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(statements, Some(TransactionEnd::Commit)),
    )
    .await;

    let expected_columns = vec![
        postgres_types::DbColumn {
            name: "id".to_string(),
            ordinal: 0,
            db_type: postgres_types::DbColumnType::Varchar,
            db_type_name: "VARCHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "enum_col".to_string(),
            ordinal: 1,
            db_type: postgres_types::DbColumnType::Enumeration(
                postgres_types::EnumerationType::new("test_enum".to_string()),
            ),
            db_type_name: "test_enum".to_string(),
        },
        postgres_types::DbColumn {
            name: "char_col".to_string(),
            ordinal: 2,
            db_type: postgres_types::DbColumnType::Character,
            db_type_name: "\"CHAR\"".to_string(),
        },
        postgres_types::DbColumn {
            name: "int2_col".to_string(),
            ordinal: 3,
            db_type: postgres_types::DbColumnType::Int2,
            db_type_name: "INT2".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4_col".to_string(),
            ordinal: 4,
            db_type: postgres_types::DbColumnType::Int4,
            db_type_name: "INT4".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8_col".to_string(),
            ordinal: 5,
            db_type: postgres_types::DbColumnType::Int8,
            db_type_name: "INT8".to_string(),
        },
        postgres_types::DbColumn {
            name: "float4_col".to_string(),
            ordinal: 6,
            db_type: postgres_types::DbColumnType::Float4,
            db_type_name: "FLOAT4".to_string(),
        },
        postgres_types::DbColumn {
            name: "float8_col".to_string(),
            ordinal: 7,
            db_type: postgres_types::DbColumnType::Float8,
            db_type_name: "FLOAT8".to_string(),
        },
        postgres_types::DbColumn {
            name: "numeric_col".to_string(),
            ordinal: 8,
            db_type: postgres_types::DbColumnType::Numeric,
            db_type_name: "NUMERIC".to_string(),
        },
        postgres_types::DbColumn {
            name: "boolean_col".to_string(),
            ordinal: 9,
            db_type: postgres_types::DbColumnType::Boolean,
            db_type_name: "BOOL".to_string(),
        },
        postgres_types::DbColumn {
            name: "text_col".to_string(),
            ordinal: 10,
            db_type: postgres_types::DbColumnType::Text,
            db_type_name: "TEXT".to_string(),
        },
        postgres_types::DbColumn {
            name: "varchar_col".to_string(),
            ordinal: 11,
            db_type: postgres_types::DbColumnType::Varchar,
            db_type_name: "VARCHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "bpchar_col".to_string(),
            ordinal: 12,
            db_type: postgres_types::DbColumnType::Bpchar,
            db_type_name: "CHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamp_col".to_string(),
            ordinal: 13,
            db_type: postgres_types::DbColumnType::Timestamp,
            db_type_name: "TIMESTAMP".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamptz_col".to_string(),
            ordinal: 14,
            db_type: postgres_types::DbColumnType::Timestamptz,
            db_type_name: "TIMESTAMPTZ".to_string(),
        },
        postgres_types::DbColumn {
            name: "date_col".to_string(),
            ordinal: 15,
            db_type: postgres_types::DbColumnType::Date,
            db_type_name: "DATE".to_string(),
        },
        postgres_types::DbColumn {
            name: "time_col".to_string(),
            ordinal: 16,
            db_type: postgres_types::DbColumnType::Time,
            db_type_name: "TIME".to_string(),
        },
        postgres_types::DbColumn {
            name: "timetz_col".to_string(),
            ordinal: 17,
            db_type: postgres_types::DbColumnType::Timetz,
            db_type_name: "TIMETZ".to_string(),
        },
        postgres_types::DbColumn {
            name: "interval_col".to_string(),
            ordinal: 18,
            db_type: postgres_types::DbColumnType::Interval,
            db_type_name: "INTERVAL".to_string(),
        },
        postgres_types::DbColumn {
            name: "bytea_col".to_string(),
            ordinal: 19,
            db_type: postgres_types::DbColumnType::Bytea,
            db_type_name: "BYTEA".to_string(),
        },
        postgres_types::DbColumn {
            name: "uuid_col".to_string(),
            ordinal: 20,
            db_type: postgres_types::DbColumnType::Uuid,
            db_type_name: "UUID".to_string(),
        },
        postgres_types::DbColumn {
            name: "json_col".to_string(),
            ordinal: 21,
            db_type: postgres_types::DbColumnType::Json,
            db_type_name: "JSON".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonb_col".to_string(),
            ordinal: 22,
            db_type: postgres_types::DbColumnType::Jsonb,
            db_type_name: "JSONB".to_string(),
        },
        postgres_types::DbColumn {
            name: "inet_col".to_string(),
            ordinal: 23,
            db_type: postgres_types::DbColumnType::Inet,
            db_type_name: "INET".to_string(),
        },
        postgres_types::DbColumn {
            name: "cidr_col".to_string(),
            ordinal: 24,
            db_type: postgres_types::DbColumnType::Cidr,
            db_type_name: "CIDR".to_string(),
        },
        postgres_types::DbColumn {
            name: "macaddr_col".to_string(),
            ordinal: 25,
            db_type: postgres_types::DbColumnType::Macaddr,
            db_type_name: "MACADDR".to_string(),
        },
        postgres_types::DbColumn {
            name: "bit_col".to_string(),
            ordinal: 26,
            db_type: postgres_types::DbColumnType::Bit,
            db_type_name: "BIT".to_string(),
        },
        postgres_types::DbColumn {
            name: "varbit_col".to_string(),
            ordinal: 27,
            db_type: postgres_types::DbColumnType::Varbit,
            db_type_name: "VARBIT".to_string(),
        },
        postgres_types::DbColumn {
            name: "xml_col".to_string(),
            ordinal: 28,
            db_type: postgres_types::DbColumnType::Xml,
            db_type_name: "XML".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4range_col".to_string(),
            ordinal: 29,
            db_type: postgres_types::DbColumnType::Int4range,
            db_type_name: "INT4RANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8range_col".to_string(),
            ordinal: 30,
            db_type: postgres_types::DbColumnType::Int8range,
            db_type_name: "INT8RANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "numrange_col".to_string(),
            ordinal: 31,
            db_type: postgres_types::DbColumnType::Numrange,
            db_type_name: "NUMRANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsrange_col".to_string(),
            ordinal: 32,
            db_type: postgres_types::DbColumnType::Tsrange,
            db_type_name: "TSRANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "tstzrange_col".to_string(),
            ordinal: 33,
            db_type: postgres_types::DbColumnType::Tstzrange,
            db_type_name: "TSTZRANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "daterange_col".to_string(),
            ordinal: 34,
            db_type: postgres_types::DbColumnType::Daterange,
            db_type_name: "DATERANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "money_col".to_string(),
            ordinal: 35,
            db_type: postgres_types::DbColumnType::Money,
            db_type_name: "MONEY".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonpath_col".to_string(),
            ordinal: 36,
            db_type: postgres_types::DbColumnType::Jsonpath,
            db_type_name: "JSONPATH".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsvector_col".to_string(),
            ordinal: 37,
            db_type: postgres_types::DbColumnType::Text,
            db_type_name: "TEXT".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsquery_col".to_string(),
            ordinal: 38,
            db_type: postgres_types::DbColumnType::Text,
            db_type_name: "TEXT".to_string(),
        },
        postgres_types::DbColumn {
            name: "inventory_item_col".to_string(),
            ordinal: 39,
            db_type: postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
                "inventory_item".to_string(),
                vec![
                    ("product_id".to_string(), postgres_types::DbColumnType::Uuid),
                    ("name".to_string(), postgres_types::DbColumnType::Text),
                    (
                        "supplier_id".to_string(),
                        postgres_types::DbColumnType::Int4,
                    ),
                    ("price".to_string(), postgres_types::DbColumnType::Numeric),
                ],
            )),
            db_type_name: "inventory_item".to_string(),
        },
        postgres_types::DbColumn {
            name: "posint4_col".to_string(),
            ordinal: 40,
            db_type: postgres_types::DbColumnType::Int4,
            db_type_name: "INT4".to_string(),
        },
        postgres_types::DbColumn {
            name: "float8range_col".to_string(),
            ordinal: 41,
            db_type: postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
                "float8range".to_string(),
                postgres_types::DbColumnType::Float8,
            )),
            db_type_name: "float8range".to_string(),
        },
    ];

    let select_statement = r#"
            SELECT
            id,
            enum_col,
            char_col,
            int2_col,
            int4_col,
            int8_col,
            float4_col,
            float8_col,
            numeric_col,
            boolean_col,
            text_col,
            varchar_col,
            bpchar_col,
            timestamp_col,
            timestamptz_col,
            date_col,
            time_col,
            timetz_col,
            interval_col,
            bytea_col,
            uuid_col,
            json_col,
            jsonb_col,
            inet_col,
            cidr_col,
            macaddr_col,
            bit_col,
            varbit_col,
            xml_col,
            int4range_col,
            int8range_col,
            numrange_col,
            tsrange_col,
            tstzrange_col,
            daterange_col,
            money_col,
            jsonpath_col,
            tsvector_col::text,
            tsquery_col::text,
            inventory_item_col,
            posint4_col,
            float8range_col
           FROM data_types ORDER BY id ASC;
        "#;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![
                StatementTest::query_stream_test(
                    select_statement,
                    vec![],
                    Some(expected_columns),
                    Some(rows),
                ),
                StatementTest::execute_test("DELETE FROM data_types;", vec![], None),
            ],
            None,
        ),
    )
    .await;
}

#[test]
async fn postgres_create_insert_select_array_test(
    postgres: &DockerPostgresRdb,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.public_connection_string();
    let rdbms = rdbms_service.postgres();

    let statements = vec![
        r#"
            CREATE TYPE a_test_enum AS ENUM ('first', 'second', 'third');
        "#,
        r#"
            CREATE TYPE a_inventory_item AS (
                product_id      uuid,
                name            text,
                supplier_id     INT4,
                price           numeric
            );
        "#,
        r#"
            CREATE TYPE float4range AS RANGE (
                subtype = float4
            );
        "#,
        "CREATE TYPE a_custom_type AS (val int4);",
        r#"
            CREATE FUNCTION a_custom_type_lt(a a_custom_type, b a_custom_type)
            RETURNS boolean AS $$
                SELECT a.val < b.val;
            $$ LANGUAGE SQL IMMUTABLE STRICT;
        "#,
        r#"
            CREATE FUNCTION a_custom_type_le(a a_custom_type, b a_custom_type)
            RETURNS boolean AS $$
                SELECT a.val <= b.val;
            $$ LANGUAGE SQL IMMUTABLE STRICT;
        "#,
        r#"
            CREATE FUNCTION a_custom_type_gt(a a_custom_type, b a_custom_type)
            RETURNS boolean AS $$
                SELECT a.val > b.val;
            $$ LANGUAGE SQL IMMUTABLE STRICT;
        "#,
        r#"
            CREATE FUNCTION a_custom_type_ge(a a_custom_type, b a_custom_type)
            RETURNS boolean AS $$
                SELECT a.val >= b.val;
            $$ LANGUAGE SQL IMMUTABLE STRICT;
        "#,
        r#"
            CREATE FUNCTION a_custom_type_eq(a a_custom_type, b a_custom_type)
            RETURNS boolean AS $$
                SELECT a.val = b.val;
            $$ LANGUAGE SQL IMMUTABLE STRICT;
        "#,
        r#"
            CREATE FUNCTION a_custom_type_ne(a a_custom_type, b a_custom_type)
            RETURNS boolean AS $$
                SELECT a.val <> b.val;
            $$ LANGUAGE SQL IMMUTABLE STRICT;
        "#,
        r#"
            CREATE OPERATOR < (
                LEFTARG = a_custom_type, RIGHTARG = a_custom_type, PROCEDURE = a_custom_type_lt
            );
        "#,
        r#"
            CREATE OPERATOR <= (
                LEFTARG = a_custom_type, RIGHTARG = a_custom_type, PROCEDURE = a_custom_type_le
            );
        "#,
        r#"
             CREATE OPERATOR > (
                LEFTARG = a_custom_type, RIGHTARG = a_custom_type, PROCEDURE = a_custom_type_gt
            );
        "#,
        r#"
            CREATE OPERATOR >= (
                LEFTARG = a_custom_type, RIGHTARG = a_custom_type, PROCEDURE = a_custom_type_ge
            );
        "#,
        r#"
            CREATE OPERATOR = (
                LEFTARG = a_custom_type, RIGHTARG = a_custom_type, PROCEDURE = a_custom_type_eq, COMMUTATOR = =
            );
        "#,
        r#"
            CREATE OPERATOR <> (
                LEFTARG = a_custom_type, RIGHTARG = a_custom_type, PROCEDURE = a_custom_type_ne, COMMUTATOR = <>
            );
        "#,
        r#"
            CREATE TYPE a_custom_type_range AS RANGE (
                subtype = a_custom_type
            );
        "#,
        r#"
            CREATE DOMAIN posint8 AS INT8 CHECK (VALUE > 0);
        "#,
        r#"
            CREATE TABLE array_data_types (
                id VARCHAR(25) PRIMARY KEY,
                enum_col a_test_enum[],
                char_col "char"[],
                int2_col INT2[],
                int4_col INT4[],
                int8_col INT8[],
                float4_col FLOAT4[],
                float8_col FLOAT8[],
                numeric_col NUMERIC(10,2)[],
                boolean_col BOOLEAN[],
                text_col TEXT[],
                varchar_col VARCHAR(255)[],
                bpchar_col CHAR(10)[],
                timestamp_col TIMESTAMP[],
                timestamptz_col TIMESTAMP WITH TIME ZONE[],
                date_col DATE[],
                time_col TIME[],
                timetz_col TIME WITH TIME ZONE[],
                interval_col INTERVAL[],
                bytea_col BYTEA[],
                uuid_col UUID[],
                json_col JSON[],
                jsonb_col JSONB[],
                inet_col INET[],
                cidr_col CIDR[],
                macaddr_col MACADDR[],
                bit_col BIT(3)[],
                varbit_col VARBIT[],
                xml_col XML[],
                int4range_col INT4RANGE[],
                int8range_col INT8RANGE[],
                numrange_col NUMRANGE[],
                tsrange_col TSRANGE[],
                tstzrange_col TSTZRANGE[],
                daterange_col DATERANGE[],
                money_col MONEY[],
                jsonpath_col JSONPATH[],
                tsvector_col TSVECTOR[],
                tsquery_col TSQUERY[],
                inventory_item_col a_inventory_item[],
                posint8_col posint8[],
                float4range_col float4range[],
                a_custom_type_range_col a_custom_type_range[]
            );
        "#,
    ];

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            statements
                .into_iter()
                .map(|s| StatementTest::execute_test(s, vec![], Some(0)))
                .collect(),
            None,
        ),
    )
    .await;

    let insert_statement = r#"
            INSERT INTO array_data_types
            (
            id,
            enum_col,
            char_col,
            int2_col,
            int4_col,
            int8_col,
            float4_col,
            float8_col,
            numeric_col,
            boolean_col,
            text_col,
            varchar_col,
            bpchar_col,
            timestamp_col,
            timestamptz_col,
            date_col,
            time_col,
            timetz_col,
            interval_col,
            bytea_col,
            uuid_col,
            json_col,
            jsonb_col,
            inet_col,
            cidr_col,
            macaddr_col,
            bit_col,
            varbit_col,
            xml_col,
            int4range_col,
            int8range_col,
            numrange_col,
            tsrange_col,
            tstzrange_col,
            daterange_col,
            money_col,
            jsonpath_col,
            tsvector_col,
            tsquery_col,
            inventory_item_col,
            posint8_col,
            float4range_col,
            a_custom_type_range_col
            )
            VALUES
            (
                $1, $2, $3, $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21, $22, $23, $24, $25, $26, $27, $28, $29,
                $30, $31, $32, $33, $34, $35, $36, $37, $38::tsvector[], $39::tsquery[],
                $40, $41, $42, $43
            );
        "#;

    let count = 4;

    let mut rows: Vec<DbRow<postgres_types::DbValue>> = Vec::with_capacity(count);
    let mut statements: Vec<StatementTest<PostgresType>> = Vec::with_capacity(count);
    for i in 0..count {
        let mut params: Vec<postgres_types::DbValue> =
            vec![postgres_types::DbValue::Varchar(format!("{i:03}"))];

        if i % 2 == 0 {
            let tstzbounds = postgres_types::ValuesRange::new(
                Bound::Included(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 3, 2 + i as u32).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                Bound::Excluded(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2024, 1, 1 + i as u32).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
            );
            let tsbounds = postgres_types::ValuesRange::new(
                Bound::Included(chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2022, 2, 2 + i as u32).unwrap(),
                    chrono::NaiveTime::from_hms_opt(16, 50, 30).unwrap(),
                )),
                Bound::Excluded(chrono::NaiveDateTime::new(
                    chrono::NaiveDate::from_ymd_opt(2024, 1, 1 + i as u32).unwrap(),
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )),
            );

            params.append(&mut vec![
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Enumeration(postgres_types::Enumeration::new(
                        "a_test_enum".to_string(),
                        "second".to_string(),
                    )),
                    postgres_types::DbValue::Enumeration(postgres_types::Enumeration::new(
                        "a_test_enum".to_string(),
                        "third".to_string(),
                    )),
                ]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Character(2)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int2(1)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int4(2)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int8(3)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Float4(4.0)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Float8(5.0)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Numeric(
                    BigDecimal::from(48888),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Boolean(true)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                    "text".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Varchar(
                    "varchar".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bpchar(
                    "0123456789".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timestamp(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timestamptz(
                    chrono::DateTime::from_naive_utc_and_offset(
                        chrono::NaiveDateTime::new(
                            chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                            chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                        ),
                        chrono::Utc,
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Date(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Time(
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Timetz(
                    postgres_types::TimeTz::new(
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                        chrono::FixedOffset::east_opt(5 * 60 * 60).unwrap(),
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Interval(
                    postgres_types::Interval::new(10, 20, 30),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bytea(
                    "bytea".as_bytes().to_vec(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Uuid(Uuid::new_v4())]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Json(
                    json!(
                           {
                              "id": i
                           }
                    )
                    .to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Jsonb(
                    json!(
                           {
                              "index": i
                           }
                    )
                    .to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Inet(IpAddr::V4(
                    Ipv4Addr::new(127, 0, 0, 1),
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Cidr(IpAddr::V6(
                    Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc00a, 0x2ff),
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Macaddr(
                    MacAddress::new([0, 1, 2, 3, 4, i as u8]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Bit(
                    BitVec::from_iter(vec![true, false, true]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Varbit(
                    BitVec::from_iter(vec![true, false, false]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Xml(format!(
                    "<foo>{i}</foo>"
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int4range(
                    postgres_types::ValuesRange::new(Bound::Included(1), Bound::Excluded(4)),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Int8range(
                    postgres_types::ValuesRange::new(Bound::Included(1), Bound::Unbounded),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Numrange(
                    postgres_types::ValuesRange::new(
                        Bound::Included(BigDecimal::from(11)),
                        Bound::Excluded(BigDecimal::from(221)),
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Tsrange(tsbounds)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Tstzrange(
                    tstzbounds,
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Daterange(
                    postgres_types::ValuesRange::new(
                        Bound::Included(
                            chrono::NaiveDate::from_ymd_opt(2023 + i as i32, 2, 3).unwrap(),
                        ),
                        Bound::Unbounded,
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Money(1234)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Jsonpath(
                    "$.user.addresses[0].city".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                    "'a' 'and' 'ate' 'cat' 'fat' 'mat' 'on' 'rat' 'sat'".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValue::Text(
                    "'fat' & 'rat' & !'cat'".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Composite(postgres_types::Composite::new(
                        "a_inventory_item".to_string(),
                        vec![
                            postgres_types::DbValue::Uuid(Uuid::new_v4()),
                            postgres_types::DbValue::Text("text".to_string()),
                            postgres_types::DbValue::Int4(i as i32),
                            postgres_types::DbValue::Numeric(BigDecimal::from(111)),
                        ],
                    )),
                    postgres_types::DbValue::Composite(postgres_types::Composite::new(
                        "a_inventory_item".to_string(),
                        vec![
                            postgres_types::DbValue::Uuid(Uuid::new_v4()),
                            postgres_types::DbValue::Text("text".to_string()),
                            postgres_types::DbValue::Int4(2 + i as i32),
                            postgres_types::DbValue::Numeric(BigDecimal::from(111)),
                        ],
                    )),
                ]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Domain(postgres_types::Domain::new(
                        "posint8".to_string(),
                        postgres_types::DbValue::Int8(1 + i as i64),
                    )),
                    postgres_types::DbValue::Domain(postgres_types::Domain::new(
                        "posint8".to_string(),
                        postgres_types::DbValue::Int8(2 + i as i64),
                    )),
                ]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "float4range".to_string(),
                        postgres_types::ValuesRange::new(Bound::Unbounded, Bound::Unbounded),
                    )),
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "float4range".to_string(),
                        postgres_types::ValuesRange::new(
                            Bound::Unbounded,
                            Bound::Excluded(postgres_types::DbValue::Float4(6.55)),
                        ),
                    )),
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "float4range".to_string(),
                        postgres_types::ValuesRange::new(
                            Bound::Included(postgres_types::DbValue::Float4(2.23)),
                            Bound::Excluded(postgres_types::DbValue::Float4(4.55)),
                        ),
                    )),
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "float4range".to_string(),
                        postgres_types::ValuesRange::new(
                            Bound::Included(postgres_types::DbValue::Float4(1.23)),
                            Bound::Unbounded,
                        ),
                    )),
                ]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "a_custom_type_range".to_string(),
                        postgres_types::ValuesRange::new(Bound::Unbounded, Bound::Unbounded),
                    )),
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "a_custom_type_range".to_string(),
                        postgres_types::ValuesRange::new(
                            Bound::Unbounded,
                            Bound::Excluded(postgres_types::DbValue::Composite(
                                postgres_types::Composite::new(
                                    "a_custom_type".to_string(),
                                    vec![postgres_types::DbValue::Int4(i as i32 + 1)],
                                ),
                            )),
                        ),
                    )),
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "a_custom_type_range".to_string(),
                        postgres_types::ValuesRange::new(
                            Bound::Included(postgres_types::DbValue::Composite(
                                postgres_types::Composite::new(
                                    "a_custom_type".to_string(),
                                    vec![postgres_types::DbValue::Int4(i as i32 + 2)],
                                ),
                            )),
                            Bound::Excluded(postgres_types::DbValue::Composite(
                                postgres_types::Composite::new(
                                    "a_custom_type".to_string(),
                                    vec![postgres_types::DbValue::Int4(i as i32 + 5)],
                                ),
                            )),
                        ),
                    )),
                    postgres_types::DbValue::Range(postgres_types::Range::new(
                        "a_custom_type_range".to_string(),
                        postgres_types::ValuesRange::new(
                            Bound::Included(postgres_types::DbValue::Composite(
                                postgres_types::Composite::new(
                                    "a_custom_type".to_string(),
                                    vec![postgres_types::DbValue::Int4(i as i32 + 1)],
                                ),
                            )),
                            Bound::Unbounded,
                        ),
                    )),
                ]),
            ]);
        } else {
            for _ in 0..42 {
                params.push(postgres_types::DbValue::Array(vec![]));
            }
        }

        statements.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        rows.push(DbRow { values: params });
    }

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(statements, Some(TransactionEnd::Commit)),
    )
    .await;

    let expected_columns = vec![
        postgres_types::DbColumn {
            name: "id".to_string(),
            ordinal: 0,
            db_type: postgres_types::DbColumnType::Varchar,
            db_type_name: "VARCHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "enum_col".to_string(),
            ordinal: 1,
            db_type: postgres_types::DbColumnType::Enumeration(
                postgres_types::EnumerationType::new("a_test_enum".to_string()),
            )
            .into_array(),
            db_type_name: "a_test_enum[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "char_col".to_string(),
            ordinal: 2,
            db_type: postgres_types::DbColumnType::Character.into_array(),
            db_type_name: "\"CHAR\"[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int2_col".to_string(),
            ordinal: 3,
            db_type: postgres_types::DbColumnType::Int2.into_array(),
            db_type_name: "INT2[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4_col".to_string(),
            ordinal: 4,
            db_type: postgres_types::DbColumnType::Int4.into_array(),
            db_type_name: "INT4[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8_col".to_string(),
            ordinal: 5,
            db_type: postgres_types::DbColumnType::Int8.into_array(),
            db_type_name: "INT8[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "float4_col".to_string(),
            ordinal: 6,
            db_type: postgres_types::DbColumnType::Float4.into_array(),
            db_type_name: "FLOAT4[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "float8_col".to_string(),
            ordinal: 7,
            db_type: postgres_types::DbColumnType::Float8.into_array(),
            db_type_name: "FLOAT8[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "numeric_col".to_string(),
            ordinal: 8,
            db_type: postgres_types::DbColumnType::Numeric.into_array(),
            db_type_name: "NUMERIC[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "boolean_col".to_string(),
            ordinal: 9,
            db_type: postgres_types::DbColumnType::Boolean.into_array(),
            db_type_name: "BOOL[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "text_col".to_string(),
            ordinal: 10,
            db_type: postgres_types::DbColumnType::Text.into_array(),
            db_type_name: "TEXT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "varchar_col".to_string(),
            ordinal: 11,
            db_type: postgres_types::DbColumnType::Varchar.into_array(),
            db_type_name: "VARCHAR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "bpchar_col".to_string(),
            ordinal: 12,
            db_type: postgres_types::DbColumnType::Bpchar.into_array(),
            db_type_name: "CHAR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamp_col".to_string(),
            ordinal: 13,
            db_type: postgres_types::DbColumnType::Timestamp.into_array(),
            db_type_name: "TIMESTAMP[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamptz_col".to_string(),
            ordinal: 14,
            db_type: postgres_types::DbColumnType::Timestamptz.into_array(),
            db_type_name: "TIMESTAMPTZ[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "date_col".to_string(),
            ordinal: 15,
            db_type: postgres_types::DbColumnType::Date.into_array(),
            db_type_name: "DATE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "time_col".to_string(),
            ordinal: 16,
            db_type: postgres_types::DbColumnType::Time.into_array(),
            db_type_name: "TIME[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "timetz_col".to_string(),
            ordinal: 17,
            db_type: postgres_types::DbColumnType::Timetz.into_array(),
            db_type_name: "TIMETZ[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "interval_col".to_string(),
            ordinal: 18,
            db_type: postgres_types::DbColumnType::Interval.into_array(),
            db_type_name: "INTERVAL[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "bytea_col".to_string(),
            ordinal: 19,
            db_type: postgres_types::DbColumnType::Bytea.into_array(),
            db_type_name: "BYTEA[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "uuid_col".to_string(),
            ordinal: 20,
            db_type: postgres_types::DbColumnType::Uuid.into_array(),
            db_type_name: "UUID[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "json_col".to_string(),
            ordinal: 21,
            db_type: postgres_types::DbColumnType::Json.into_array(),
            db_type_name: "JSON[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonb_col".to_string(),
            ordinal: 22,
            db_type: postgres_types::DbColumnType::Jsonb.into_array(),
            db_type_name: "JSONB[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "inet_col".to_string(),
            ordinal: 23,
            db_type: postgres_types::DbColumnType::Inet.into_array(),
            db_type_name: "INET[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "cidr_col".to_string(),
            ordinal: 24,
            db_type: postgres_types::DbColumnType::Cidr.into_array(),
            db_type_name: "CIDR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "macaddr_col".to_string(),
            ordinal: 25,
            db_type: postgres_types::DbColumnType::Macaddr.into_array(),
            db_type_name: "MACADDR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "bit_col".to_string(),
            ordinal: 26,
            db_type: postgres_types::DbColumnType::Bit.into_array(),
            db_type_name: "BIT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "varbit_col".to_string(),
            ordinal: 27,
            db_type: postgres_types::DbColumnType::Varbit.into_array(),
            db_type_name: "VARBIT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "xml_col".to_string(),
            ordinal: 28,
            db_type: postgres_types::DbColumnType::Xml.into_array(),
            db_type_name: "XML[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4range_col".to_string(),
            ordinal: 29,
            db_type: postgres_types::DbColumnType::Int4range.into_array(),
            db_type_name: "INT4RANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8range_col".to_string(),
            ordinal: 30,
            db_type: postgres_types::DbColumnType::Int8range.into_array(),
            db_type_name: "INT8RANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "numrange_col".to_string(),
            ordinal: 31,
            db_type: postgres_types::DbColumnType::Numrange.into_array(),
            db_type_name: "NUMRANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsrange_col".to_string(),
            ordinal: 32,
            db_type: postgres_types::DbColumnType::Tsrange.into_array(),
            db_type_name: "TSRANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tstzrange_col".to_string(),
            ordinal: 33,
            db_type: postgres_types::DbColumnType::Tstzrange.into_array(),
            db_type_name: "TSTZRANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "daterange_col".to_string(),
            ordinal: 34,
            db_type: postgres_types::DbColumnType::Daterange.into_array(),
            db_type_name: "DATERANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "money_col".to_string(),
            ordinal: 35,
            db_type: postgres_types::DbColumnType::Money.into_array(),
            db_type_name: "MONEY[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonpath_col".to_string(),
            ordinal: 36,
            db_type: postgres_types::DbColumnType::Jsonpath.into_array(),
            db_type_name: "JSONPATH[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsvector_col".to_string(),
            ordinal: 37,
            db_type: postgres_types::DbColumnType::Text.into_array(),
            db_type_name: "TEXT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsquery_col".to_string(),
            ordinal: 38,
            db_type: postgres_types::DbColumnType::Text.into_array(),
            db_type_name: "TEXT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "inventory_item_col".to_string(),
            ordinal: 39,
            db_type: postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
                "a_inventory_item".to_string(),
                vec![
                    ("product_id".to_string(), postgres_types::DbColumnType::Uuid),
                    ("name".to_string(), postgres_types::DbColumnType::Text),
                    (
                        "supplier_id".to_string(),
                        postgres_types::DbColumnType::Int4,
                    ),
                    ("price".to_string(), postgres_types::DbColumnType::Numeric),
                ],
            ))
            .into_array(),
            db_type_name: "a_inventory_item[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "posint8_col".to_string(),
            ordinal: 40,
            db_type: postgres_types::DbColumnType::Domain(postgres_types::DomainType::new(
                "posint8".to_string(),
                postgres_types::DbColumnType::Int8,
            ))
            .into_array(),
            db_type_name: "posint8[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "float4range_col".to_string(),
            ordinal: 41,
            db_type: postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
                "float4range".to_string(),
                postgres_types::DbColumnType::Float4,
            ))
            .into_array(),
            db_type_name: "float4range[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "a_custom_type_range_col".to_string(),
            ordinal: 42,
            db_type: postgres_types::DbColumnType::Range(postgres_types::RangeType::new(
                "a_custom_type_range".to_string(),
                postgres_types::DbColumnType::Composite(postgres_types::CompositeType::new(
                    "a_custom_type".to_string(),
                    vec![("val".to_string(), postgres_types::DbColumnType::Int4)],
                )),
            ))
            .into_array(),
            db_type_name: "a_custom_type_range[]".to_string(),
        },
    ];

    let select_statement = r#"
            SELECT
            id,
            enum_col,
            char_col,
            int2_col,
            int4_col,
            int8_col,
            float4_col,
            float8_col,
            numeric_col,
            boolean_col,
            text_col,
            varchar_col,
            bpchar_col,
            timestamp_col,
            timestamptz_col,
            date_col,
            time_col,
            timetz_col,
            interval_col,
            bytea_col,
            uuid_col,
            json_col,
            jsonb_col,
            inet_col,
            cidr_col,
            macaddr_col,
            bit_col,
            varbit_col,
            xml_col,
            int4range_col,
            int8range_col,
            numrange_col,
            tsrange_col,
            tstzrange_col,
            daterange_col,
            money_col,
            jsonpath_col,
            tsvector_col::text[],
            tsquery_col::text[],
            inventory_item_col,
            posint8_col,
            float4range_col,
            a_custom_type_range_col
           FROM array_data_types ORDER BY id ASC;
        "#;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![
                StatementTest::query_stream_test(
                    select_statement,
                    vec![],
                    Some(expected_columns),
                    Some(rows),
                ),
                StatementTest::execute_test("DELETE FROM array_data_types;", vec![], None),
            ],
            None,
        ),
    )
    .await;
}

#[test]
async fn postgres_schema_test(postgres: &DockerPostgresRdb, rdbms_service: &RdbmsServiceDefault) {
    let rdbms = rdbms_service.postgres();
    let db_address = postgres.public_connection_string();

    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS test1.components
            (
                component_id        varchar(255)    NOT NULL,
                namespace           varchar(255)    NOT NULL,
                name                varchar(255)    NOT NULL,
                PRIMARY KEY (component_id)
            );
        "#;
    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![
                StatementTest::execute_test("CREATE SCHEMA IF NOT EXISTS test1;", vec![], None),
                StatementTest::execute_test(create_table_statement, vec![], None),
                StatementTest::execute_test(
                    "SELECT component_id, namespace, name from test1.components",
                    vec![],
                    Some(0),
                ),
            ],
            None,
        ),
    )
    .await;
}

#[test]
async fn mysql_transaction_tests(mysql: &DockerMysqlRdb, rdbms_service: &RdbmsServiceDefault) {
    let db_address = mysql.public_connection_string();
    let rdbms = rdbms_service.mysql();

    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS test_users
            (
                user_id             varchar(25)    NOT NULL,
                name                varchar(255)    NOT NULL,
                created_on          timestamp NOT NULL DEFAULT NOW(),
                PRIMARY KEY (user_id)
            );
        "#;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![StatementTest::execute_test(
                create_table_statement,
                vec![],
                None,
            )],
            None,
        ),
    )
    .await;

    let insert_statement = r#"
            INSERT INTO test_users
            (user_id, name)
            VALUES
            (?, ?)
        "#;

    let count = 60;

    let mut rows: Vec<DbRow<mysql_types::DbValue>> = Vec::with_capacity(count);

    let mut statements: Vec<StatementTest<MysqlType>> = Vec::with_capacity(count);

    for i in 0..count {
        let params: Vec<mysql_types::DbValue> = vec![
            mysql_types::DbValue::Varchar(format!("{i:03}")),
            mysql_types::DbValue::Varchar(format!("name-{i:03}")),
        ];

        statements.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        rows.push(DbRow { values: params });
    }

    let select_statement_test = StatementTest::query_test(
        "SELECT user_id, name FROM test_users ORDER BY user_id ASC",
        vec![],
        None,
        Some(rows.clone()),
    );

    statements.push(select_statement_test.clone());

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(statements, Some(TransactionEnd::Commit)),
    )
    .await;

    let delete_statement_test = StatementTest::execute_test("DELETE FROM test_users", vec![], None);

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![delete_statement_test.clone()],
            Some(TransactionEnd::Rollback),
        ),
    )
    .await;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![select_statement_test.with_query_stream_expected(None, Some(rows.clone()))],
            Some(TransactionEnd::Commit),
        ),
    )
    .await;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(vec![delete_statement_test], Some(TransactionEnd::Commit)),
    )
    .await;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![select_statement_test.with_query_expected(Some(vec![]), Some(vec![]))],
            Some(TransactionEnd::None),
        ),
    )
    .await;
}

#[test]
async fn mysql_create_insert_select_test(
    mysql: &DockerMysqlRdb,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = mysql.public_connection_string();
    let rdbms = rdbms_service.mysql();
    let create_table_statement = r#"
            CREATE TABLE IF NOT EXISTS data_types
            (
              `id` VARCHAR(25) PRIMARY KEY,
              `tinyint_col` TINYINT,
              `smallint_col` SMALLINT,
              `mediumint_col` MEDIUMINT,
              `int_col` INT,
              `bigint_col` BIGINT,
              `float_col` FLOAT,
              `double_col` DOUBLE,
              `decimal_col` DECIMAL(10,2),
              `date_col` DATE,
              `datetime_col` DATETIME,
              `timestamp_col` TIMESTAMP,
              `year_col` YEAR,
              `char_col` CHAR(10),
              `varchar_col` VARCHAR(255),
              `tinytext_col` TINYTEXT,
              `text_col` TEXT,
              `mediumtext_col` MEDIUMTEXT,
              `longtext_col` LONGTEXT,
              `binary_col` BINARY(6),
              `varbinary_col` VARBINARY(255),
              `tinyblob_col` TINYBLOB,
              `blob_col` BLOB,
              `mediumblob_col` MEDIUMBLOB,
              `longblob_col` LONGBLOB,
              `enum_col` ENUM('value1', 'value2', 'value3'),
              `set_col` SET('value1', 'value2', 'value3'),
              `json_col` JSON,
              `bit_col` BIT(3),
              `tinyint_unsigned_col` TINYINT UNSIGNED,
              `smallint_unsigned_col` SMALLINT UNSIGNED,
              `mediumint_unsigned_col` MEDIUMINT UNSIGNED,
              `int_unsigned_col` INT UNSIGNED,
              `bigint_unsigned_col` BIGINT UNSIGNED,
              `time_col` TIME
            );
        "#;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![StatementTest::execute_test(
                create_table_statement,
                vec![],
                None,
            )],
            None,
        ),
    )
    .await;

    let insert_statement = r#"
            INSERT INTO data_types (
              id,
              tinyint_col, smallint_col, mediumint_col, int_col, bigint_col,
              float_col, double_col, decimal_col, date_col, datetime_col,
              timestamp_col, char_col, varchar_col, tinytext_col,
              text_col, mediumtext_col, longtext_col, binary_col, varbinary_col,
              tinyblob_col, blob_col, mediumblob_col, longblob_col, enum_col,
              set_col, json_col, bit_col,
              tinyint_unsigned_col, smallint_unsigned_col,
              mediumint_unsigned_col, int_unsigned_col, bigint_unsigned_col,
              year_col, time_col
            ) VALUES (
              ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?, ?
            );
        "#;

    let count = 4;

    let mut rows: Vec<DbRow<mysql_types::DbValue>> = Vec::with_capacity(count);
    let mut statements: Vec<StatementTest<MysqlType>> = Vec::with_capacity(count);
    for i in 0..count {
        let mut params: Vec<mysql_types::DbValue> =
            vec![mysql_types::DbValue::Varchar(format!("{i:03}"))];

        if i % 2 == 0 {
            params.append(&mut vec![
                mysql_types::DbValue::Tinyint(1),
                mysql_types::DbValue::Smallint(2),
                mysql_types::DbValue::Mediumint(3),
                mysql_types::DbValue::Int(4),
                mysql_types::DbValue::Bigint(5),
                mysql_types::DbValue::Float(6.0),
                mysql_types::DbValue::Double(7.0),
                mysql_types::DbValue::Decimal(BigDecimal::from_str("80.00").unwrap()),
                mysql_types::DbValue::Date(
                    chrono::NaiveDate::from_ymd_opt(2030 + i as i32, 10, 12).unwrap(),
                ),
                mysql_types::DbValue::Datetime(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023 + i as i32, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                mysql_types::DbValue::Timestamp(chrono::DateTime::from_naive_utc_and_offset(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023 + i as i32, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                    chrono::Utc,
                )),
                mysql_types::DbValue::Fixchar("0123456789".to_string()),
                mysql_types::DbValue::Varchar(format!("name-{}", Uuid::new_v4())),
                mysql_types::DbValue::Tinytext("Tinytext".to_string()),
                mysql_types::DbValue::Text("text".to_string()),
                mysql_types::DbValue::Mediumtext("Mediumtext".to_string()),
                mysql_types::DbValue::Longtext("Longtext".to_string()),
                mysql_types::DbValue::Binary(vec![66, 105, 110, 97, 114, 121]),
                mysql_types::DbValue::Varbinary("Varbinary".as_bytes().to_vec()),
                mysql_types::DbValue::Tinyblob("Tinyblob".as_bytes().to_vec()),
                mysql_types::DbValue::Blob("Blob".as_bytes().to_vec()),
                mysql_types::DbValue::Mediumblob("Mediumblob".as_bytes().to_vec()),
                mysql_types::DbValue::Longblob("Longblob".as_bytes().to_vec()),
                mysql_types::DbValue::Enumeration("value2".to_string()),
                mysql_types::DbValue::Set("value1,value2".to_string()),
                mysql_types::DbValue::Json(
                    json!(
                           {
                              "id": i
                           }
                    )
                    .to_string(),
                ),
                mysql_types::DbValue::Bit(BitVec::from_iter([true, false, false])),
                mysql_types::DbValue::TinyintUnsigned(10),
                mysql_types::DbValue::SmallintUnsigned(20),
                mysql_types::DbValue::MediumintUnsigned(30),
                mysql_types::DbValue::IntUnsigned(40),
                mysql_types::DbValue::BigintUnsigned(50),
                mysql_types::DbValue::Year(2020),
                mysql_types::DbValue::Time(chrono::NaiveTime::from_hms_opt(1, 20, 30).unwrap()),
            ]);
        } else {
            for _ in 0..34 {
                params.push(mysql_types::DbValue::Null);
            }
        };

        statements.push(StatementTest::execute_test(
            insert_statement,
            params.clone(),
            Some(1),
        ));

        let values = params
            .into_iter()
            .map(|v| match v {
                mysql_types::DbValue::Mediumtext(v) => mysql_types::DbValue::Text(v),
                mysql_types::DbValue::Longtext(v) => mysql_types::DbValue::Text(v),
                mysql_types::DbValue::Tinytext(v) => mysql_types::DbValue::Text(v),
                mysql_types::DbValue::Mediumblob(v) => mysql_types::DbValue::Blob(v),
                mysql_types::DbValue::Longblob(v) => mysql_types::DbValue::Blob(v),
                mysql_types::DbValue::Tinyblob(v) => mysql_types::DbValue::Blob(v),
                mysql_types::DbValue::Set(v) => mysql_types::DbValue::Fixchar(v),
                _ => v,
            })
            .collect();

        rows.push(DbRow { values });
    }

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(statements, Some(TransactionEnd::Commit)),
    )
    .await;

    let expected_columns = vec![
        mysql_types::DbColumn {
            name: "id".to_string(),
            ordinal: 0,
            db_type: mysql_types::DbColumnType::Varchar,
            db_type_name: "VARCHAR".to_string(),
        },
        mysql_types::DbColumn {
            name: "tinyint_col".to_string(),
            ordinal: 1,
            db_type: mysql_types::DbColumnType::Tinyint,
            db_type_name: "TINYINT".to_string(),
        },
        mysql_types::DbColumn {
            name: "smallint_col".to_string(),
            ordinal: 2,
            db_type: mysql_types::DbColumnType::Smallint,
            db_type_name: "SMALLINT".to_string(),
        },
        mysql_types::DbColumn {
            name: "mediumint_col".to_string(),
            ordinal: 3,
            db_type: mysql_types::DbColumnType::Mediumint,
            db_type_name: "MEDIUMINT".to_string(),
        },
        mysql_types::DbColumn {
            name: "int_col".to_string(),
            ordinal: 4,
            db_type: mysql_types::DbColumnType::Int,
            db_type_name: "INT".to_string(),
        },
        mysql_types::DbColumn {
            name: "bigint_col".to_string(),
            ordinal: 5,
            db_type: mysql_types::DbColumnType::Bigint,
            db_type_name: "BIGINT".to_string(),
        },
        mysql_types::DbColumn {
            name: "float_col".to_string(),
            ordinal: 6,
            db_type: mysql_types::DbColumnType::Float,
            db_type_name: "FLOAT".to_string(),
        },
        mysql_types::DbColumn {
            name: "double_col".to_string(),
            ordinal: 7,
            db_type: mysql_types::DbColumnType::Double,
            db_type_name: "DOUBLE".to_string(),
        },
        mysql_types::DbColumn {
            name: "decimal_col".to_string(),
            ordinal: 8,
            db_type: mysql_types::DbColumnType::Decimal,
            db_type_name: "DECIMAL".to_string(),
        },
        mysql_types::DbColumn {
            name: "date_col".to_string(),
            ordinal: 9,
            db_type: mysql_types::DbColumnType::Date,
            db_type_name: "DATE".to_string(),
        },
        mysql_types::DbColumn {
            name: "datetime_col".to_string(),
            ordinal: 10,
            db_type: mysql_types::DbColumnType::Datetime,
            db_type_name: "DATETIME".to_string(),
        },
        mysql_types::DbColumn {
            name: "timestamp_col".to_string(),
            ordinal: 11,
            db_type: mysql_types::DbColumnType::Timestamp,
            db_type_name: "TIMESTAMP".to_string(),
        },
        mysql_types::DbColumn {
            name: "char_col".to_string(),
            ordinal: 12,
            db_type: mysql_types::DbColumnType::Fixchar,
            db_type_name: "CHAR".to_string(),
        },
        mysql_types::DbColumn {
            name: "varchar_col".to_string(),
            ordinal: 13,
            db_type: mysql_types::DbColumnType::Varchar,
            db_type_name: "VARCHAR".to_string(),
        },
        mysql_types::DbColumn {
            name: "tinytext_col".to_string(),
            ordinal: 14,
            db_type: mysql_types::DbColumnType::Text,
            db_type_name: "TEXT".to_string(),
        },
        mysql_types::DbColumn {
            name: "text_col".to_string(),
            ordinal: 15,
            db_type: mysql_types::DbColumnType::Text,
            db_type_name: "TEXT".to_string(),
        },
        mysql_types::DbColumn {
            name: "mediumtext_col".to_string(),
            ordinal: 16,
            db_type: mysql_types::DbColumnType::Text,
            db_type_name: "TEXT".to_string(),
        },
        mysql_types::DbColumn {
            name: "longtext_col".to_string(),
            ordinal: 17,
            db_type: mysql_types::DbColumnType::Text,
            db_type_name: "TEXT".to_string(),
        },
        mysql_types::DbColumn {
            name: "binary_col".to_string(),
            ordinal: 18,
            db_type: mysql_types::DbColumnType::Binary,
            db_type_name: "BINARY".to_string(),
        },
        mysql_types::DbColumn {
            name: "varbinary_col".to_string(),
            ordinal: 19,
            db_type: mysql_types::DbColumnType::Varbinary,
            db_type_name: "VARBINARY".to_string(),
        },
        mysql_types::DbColumn {
            name: "tinyblob_col".to_string(),
            ordinal: 20,
            db_type: mysql_types::DbColumnType::Blob,
            db_type_name: "BLOB".to_string(),
        },
        mysql_types::DbColumn {
            name: "blob_col".to_string(),
            ordinal: 21,
            db_type: mysql_types::DbColumnType::Blob,
            db_type_name: "BLOB".to_string(),
        },
        mysql_types::DbColumn {
            name: "mediumblob_col".to_string(),
            ordinal: 22,
            db_type: mysql_types::DbColumnType::Blob,
            db_type_name: "BLOB".to_string(),
        },
        mysql_types::DbColumn {
            name: "longblob_col".to_string(),
            ordinal: 23,
            db_type: mysql_types::DbColumnType::Blob,
            db_type_name: "BLOB".to_string(),
        },
        mysql_types::DbColumn {
            name: "enum_col".to_string(),
            ordinal: 24,
            db_type: mysql_types::DbColumnType::Enumeration,
            db_type_name: "ENUM".to_string(),
        },
        mysql_types::DbColumn {
            name: "set_col".to_string(),
            ordinal: 25,
            db_type: mysql_types::DbColumnType::Fixchar,
            db_type_name: "CHAR".to_string(),
        },
        mysql_types::DbColumn {
            name: "json_col".to_string(),
            ordinal: 26,
            db_type: mysql_types::DbColumnType::Json,
            db_type_name: "JSON".to_string(),
        },
        mysql_types::DbColumn {
            name: "bit_col".to_string(),
            ordinal: 27,
            db_type: mysql_types::DbColumnType::Bit,
            db_type_name: "BIT".to_string(),
        },
        mysql_types::DbColumn {
            name: "tinyint_unsigned_col".to_string(),
            ordinal: 28,
            db_type: mysql_types::DbColumnType::TinyintUnsigned,
            db_type_name: "TINYINT UNSIGNED".to_string(),
        },
        mysql_types::DbColumn {
            name: "smallint_unsigned_col".to_string(),
            ordinal: 29,
            db_type: mysql_types::DbColumnType::SmallintUnsigned,
            db_type_name: "SMALLINT UNSIGNED".to_string(),
        },
        mysql_types::DbColumn {
            name: "mediumint_unsigned_col".to_string(),
            ordinal: 30,
            db_type: mysql_types::DbColumnType::MediumintUnsigned,
            db_type_name: "MEDIUMINT UNSIGNED".to_string(),
        },
        mysql_types::DbColumn {
            name: "int_unsigned_col".to_string(),
            ordinal: 31,
            db_type: mysql_types::DbColumnType::IntUnsigned,
            db_type_name: "INT UNSIGNED".to_string(),
        },
        mysql_types::DbColumn {
            name: "bigint_unsigned_col".to_string(),
            ordinal: 32,
            db_type: mysql_types::DbColumnType::BigintUnsigned,
            db_type_name: "BIGINT UNSIGNED".to_string(),
        },
        mysql_types::DbColumn {
            name: "year_col".to_string(),
            ordinal: 33,
            db_type: mysql_types::DbColumnType::Year,
            db_type_name: "YEAR".to_string(),
        },
        mysql_types::DbColumn {
            name: "time_col".to_string(),
            ordinal: 34,
            db_type: mysql_types::DbColumnType::Time,
            db_type_name: "TIME".to_string(),
        },
    ];

    let select_statement = r#"
            SELECT
              id,
              tinyint_col, smallint_col, mediumint_col, int_col, bigint_col,
              float_col, double_col, decimal_col, date_col, datetime_col,
              timestamp_col, char_col, varchar_col, tinytext_col,
              text_col, mediumtext_col, longtext_col, binary_col, varbinary_col,
              tinyblob_col, blob_col, mediumblob_col, longblob_col, enum_col,
              set_col, json_col, bit_col,
              tinyint_unsigned_col, smallint_unsigned_col,
              mediumint_unsigned_col, int_unsigned_col, bigint_unsigned_col,
              year_col, time_col
           FROM data_types ORDER BY id ASC;
        "#;

    rdbms_test(
        rdbms.clone(),
        &db_address,
        RdbmsTest::new(
            vec![
                StatementTest::query_stream_test(
                    select_statement,
                    vec![],
                    Some(expected_columns),
                    Some(rows),
                ),
                StatementTest::execute_test("DELETE FROM data_types;", vec![], None),
            ],
            None,
        ),
    )
    .await;
}

async fn execute_rdbms_test<T: RdbmsType + 'static>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    pool_key: &RdbmsPoolKey,
    worker_id: &WorkerId,
    test: RdbmsTest<T>,
) -> (
    Option<TransactionId>,
    Vec<Result<StatementResult<T>, Error>>,
) {
    let mut results: Vec<Result<StatementResult<T>, Error>> =
        Vec::with_capacity(test.statements.len());
    let mut transaction_id: Option<TransactionId> = None;
    if let Some(te) = test.transaction_end {
        let transaction = rdbms
            .begin_transaction(pool_key, worker_id)
            .await
            .expect("New transaction expected");
        transaction_id = Some(transaction.transaction_id());
        for st in test.statements {
            match st.action {
                StatementAction::Execute(_) => {
                    let result = transaction.execute(st.statement, st.params).await;
                    results.push(result.map(StatementResult::Execute));
                }
                StatementAction::Query(_) => {
                    let result = transaction.query(st.statement, st.params).await;
                    results.push(result.map(StatementResult::Query));
                }
                StatementAction::QueryStream(_) => {
                    match transaction.query_stream(st.statement, st.params).await {
                        Ok(result_set) => {
                            let result = DbResult::from(result_set).await;
                            results.push(result.map(StatementResult::Query));
                        }
                        Err(e) => {
                            results.push(Err(e));
                        }
                    }
                }
            }
        }
        match te {
            TransactionEnd::Commit => {
                transaction
                    .pre_commit()
                    .await
                    .expect("Transaction pre commit");
                transaction.commit().await.expect("Transaction commit")
            }
            TransactionEnd::Rollback => {
                transaction
                    .pre_rollback()
                    .await
                    .expect("Transaction pre rollback");
                transaction.rollback().await.expect("Transaction rollback")
            }
            TransactionEnd::None => (),
        }
    } else {
        for st in test.statements {
            match st.action {
                StatementAction::Execute(_) => {
                    let result = rdbms
                        .execute(pool_key, worker_id, st.statement, st.params)
                        .await;
                    results.push(result.map(StatementResult::Execute));
                }
                StatementAction::Query(_) => {
                    let result = rdbms
                        .query(pool_key, worker_id, st.statement, st.params)
                        .await;
                    results.push(result.map(StatementResult::Query));
                }
                StatementAction::QueryStream(_) => {
                    match rdbms
                        .query_stream(pool_key, worker_id, st.statement, st.params)
                        .await
                    {
                        Ok(result_set) => {
                            let result = DbResult::from(result_set).await;
                            results.push(result.map(StatementResult::Query));
                        }
                        Err(e) => {
                            results.push(Err(e));
                        }
                    }
                }
            }
        }
    }

    (transaction_id, results)
}

async fn rdbms_test<T: RdbmsType + 'static>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    test: RdbmsTest<T>,
) {
    let worker_id = new_worker_id();
    let connection = rdbms.create(db_address, &worker_id).await;
    check!(connection.is_ok(), "connection to {} is ok", db_address);
    let pool_key = connection.unwrap();
    let (transaction_id, results) =
        execute_rdbms_test::<T>(rdbms.clone(), &pool_key, &worker_id, test.clone()).await;

    check_transaction(
        rdbms.clone(),
        &pool_key,
        &worker_id,
        test.transaction_end.clone(),
        transaction_id,
    )
    .await;

    check_test_results::<T>(&worker_id, test, results);

    let _ = rdbms.remove(&pool_key, &worker_id).await;
    let exists = rdbms.exists(&pool_key, &worker_id).await;
    check!(!exists);
}

async fn check_transaction<T: RdbmsType + 'static>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    pool_key: &RdbmsPoolKey,
    worker_id: &WorkerId,
    transaction_end: Option<TransactionEnd>,
    transaction_id: Option<TransactionId>,
) {
    if let Some(te) = transaction_end {
        check!(
            transaction_id.is_some(),
            "transaction id for worker {worker_id} is some"
        );
        let transaction_id = transaction_id.unwrap();
        let transaction_status = rdbms
            .get_transaction_status(pool_key, worker_id, &transaction_id)
            .await;
        check!(
            transaction_status.is_ok(),
            "transaction status for worker {worker_id} is ok"
        );
        let transaction_status = transaction_status.unwrap();
        match te {
            TransactionEnd::Commit => {
                check!(
                    transaction_status == RdbmsTransactionStatus::Committed,
                    "transaction status for worker {worker_id} is committed"
                );
            }
            TransactionEnd::Rollback => {
                check!(
                    transaction_status == RdbmsTransactionStatus::RolledBack,
                    "transaction status for worker {worker_id} is rolled back"
                );
            }
            TransactionEnd::None => (),
        }

        let result = rdbms
            .cleanup_transaction(pool_key, worker_id, &transaction_id)
            .await;
        check!(
            result.is_ok(),
            "transaction cleanup for worker {worker_id} is ok"
        );

        if PostgresType.type_id() != TypeId::of::<T>() {
            let transaction_status = rdbms
                .get_transaction_status(pool_key, worker_id, &transaction_id)
                .await;
            check!(
                transaction_status.is_ok(),
                "transaction status for worker {worker_id} is ok"
            );
            let transaction_status = transaction_status.unwrap();
            check!(
                transaction_status == RdbmsTransactionStatus::NotFound,
                "transaction status for worker {worker_id} is cleaned up"
            );
        }
    }
}

fn check_test_results<T: RdbmsType>(
    worker_id: &WorkerId,
    test: RdbmsTest<T>,
    results: Vec<Result<StatementResult<T>, Error>>,
) {
    for (i, st) in test.statements.into_iter().enumerate() {
        match st.action {
            StatementAction::Execute(expected) if expected.expected.is_some() => {
                match results.get(i).cloned() {
                    Some(Ok(StatementResult::Execute(result))) => {
                        if let Some(expected) = expected.expected {
                            check!(
                                result == expected,
                                "execute result for worker {worker_id} and test statement with index {i} match"
                            );
                        }
                    }
                    v => {
                        info!("execute result for worker {worker_id} and test statement with index {i}, statement: {}, error: {:?}", st.statement, v);
                        check!(false, "execute result for worker {worker_id} and test statement with index {i} is error or not found");
                    }
                }
            }
            StatementAction::Query(expected)
                if expected.expected_columns.is_some() || expected.expected_rows.is_some() =>
            {
                match results.get(i).cloned() {
                    Some(Ok(StatementResult::Query(result))) => {
                        if let Some(expected_columns) = expected.expected_columns {
                            check!(result.columns == expected_columns, "query result columns for worker {worker_id} and test statement with index {i} match");
                        }
                        if let Some(expected_rows) = expected.expected_rows {
                            check!(result.rows == expected_rows, "query result rows for worker {worker_id} and test statement with index {i} match");
                        }
                    }
                    v => {
                        info!("query result for worker {worker_id} and test statement with index {i}, statement: {}, error: {:?}", st.statement, v);
                        check!(false, "query result for worker {worker_id} and test statement with index {i} is error or not found");
                    }
                }
            }
            StatementAction::QueryStream(expected)
                if expected.expected_columns.is_some() || expected.expected_rows.is_some() =>
            {
                match results.get(i).cloned() {
                    Some(Ok(StatementResult::Query(result))) => {
                        if let Some(expected_columns) = expected.expected_columns {
                            check!(result.columns == expected_columns, "query stream result columns for worker {worker_id} and test statement with index {i} match");
                        }
                        if let Some(expected_rows) = expected.expected_rows {
                            check!(result.rows == expected_rows, "query stream result rows for worker {worker_id} and test statement with index {i} match");
                        }
                    }
                    v => {
                        info!("query stream result for worker {worker_id} and test statement with index {i}, statement: {}, error: {:?}", st.statement, v);
                        check!(false, "query stream result for worker {worker_id} and test statement with index {i} is error or not found");
                    }
                }
            }
            _ => (),
        }
    }
}

#[test]
async fn postgres_connection_err_test(rdbms_service: &RdbmsServiceDefault) {
    rdbms_connection_err_test(
        rdbms_service.postgres(),
        "pg://user:password@localhost:3506",
        Error::ConnectionFailure("scheme 'pg' in url is invalid".to_string()),
    )
    .await;

    rdbms_connection_err_test(
        rdbms_service.postgres(),
        "postgres://user:password@localhost:5999",
        Error::ConnectionFailure("pool timed out while waiting for an open connection".to_string()),
    )
    .await
}

#[test]
async fn postgres_query_err_test(
    postgres: &DockerPostgresRdb,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.public_connection_string();
    let rdbms = rdbms_service.postgres();

    rdbms_query_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT * FROM xxx",
        vec![],
        Error::QueryExecutionFailure(
            "error returned from database: relation \"xxx\" does not exist".to_string(),
        ),
    )
    .await;
    rdbms_query_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT 'a fat cat sat on a mat and ate a fat rat'::tsvector;",
        vec![],
        Error::QueryResponseFailure("Column type 'TSVECTOR' is not supported".to_string()),
    )
    .await;

    rdbms_query_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT 1",
        vec![postgres_types::DbValue::Array(vec![
            postgres_types::DbValue::Text("tag1".to_string()),
            postgres_types::DbValue::Int8(0),
        ])],
        Error::QueryParameterFailure(
            "Array element '0' with index 1 has different type than expected (value do not have 'text' type)".to_string(),
        ),
    )
    .await;
}

#[test]
async fn postgres_execute_err_test(
    postgres: &DockerPostgresRdb,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.public_connection_string();
    let rdbms = rdbms_service.postgres();

    rdbms_execute_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT * FROM xxx",
        vec![],
        Error::QueryExecutionFailure(
            "error returned from database: relation \"xxx\" does not exist".to_string(),
        ),
    )
    .await;

    rdbms_execute_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT 1",
        vec![postgres_types::DbValue::Array(vec![
            postgres_types::DbValue::Text("tag1".to_string()),
            postgres_types::DbValue::Int8(0),
        ])],
        Error::QueryParameterFailure(
            "Array element '0' with index 1 has different type than expected (value do not have 'text' type)".to_string(),
        ),
    )
    .await;
}

#[test]
async fn mysql_query_err_test(mysql: &DockerMysqlRdb, rdbms_service: &RdbmsServiceDefault) {
    let db_address = mysql.public_connection_string();
    let rdbms = rdbms_service.mysql();

    rdbms_query_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT * FROM xxx",
        vec![],
        Error::QueryExecutionFailure(
            "error returned from database: 1146 (42S02): Table 'mysql.xxx' doesn't exist"
                .to_string(),
        ),
    )
    .await;

    rdbms_query_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT Point(56.7, 53.34);",
        vec![],
        Error::QueryResponseFailure("Column type 'GEOMETRY' is not supported".to_string()),
    )
    .await;
}

#[test]
async fn mysql_execute_err_test(mysql: &DockerMysqlRdb, rdbms_service: &RdbmsServiceDefault) {
    let db_address = mysql.public_connection_string();
    let rdbms = rdbms_service.mysql();

    rdbms_execute_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT * FROM xxx",
        vec![],
        Error::QueryExecutionFailure(
            "error returned from database: 1146 (42S02): Table 'mysql.xxx' doesn't exist"
                .to_string(),
        ),
    )
    .await;
}

#[test]
async fn mysql_connection_err_test(rdbms_service: &RdbmsServiceDefault) {
    rdbms_connection_err_test(
        rdbms_service.mysql(),
        "msql://user:password@localhost:3506",
        Error::ConnectionFailure("scheme 'msql' in url is invalid".to_string()),
    )
    .await;

    rdbms_connection_err_test(
        rdbms_service.mysql(),
        "mysql://user:password@localhost:3506",
        Error::ConnectionFailure("pool timed out while waiting for an open connection".to_string()),
    )
    .await;
}

async fn rdbms_connection_err_test<T: RdbmsType>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    expected: Error,
) {
    let worker_id = new_worker_id();
    let result = rdbms.create(db_address, &worker_id).await;

    check!(result.is_err(), "connection to {db_address} is error");

    let error = result.err().unwrap();
    check!(
        error == expected,
        "connection error for {db_address} - response error match"
    );
}

async fn rdbms_query_err_test<T: RdbmsType>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    query: &str,
    params: Vec<T::DbValue>,
    expected: Error,
) {
    let worker_id = new_worker_id();
    let connection = rdbms.create(db_address, &worker_id).await;
    check!(connection.is_ok(), "connection to {} is ok", db_address);
    let pool_key = connection.unwrap();

    let result = rdbms
        .query_stream(&pool_key, &worker_id, query, params)
        .await;

    check!(
        result.is_err(),
        "query {} (executed on {}) - result is error",
        query,
        pool_key
    );

    let error = result.err().unwrap();
    check!(
        error == expected,
        "query {} (executed on {}) - result error match",
        query,
        pool_key
    );

    let _ = rdbms.remove(&pool_key, &worker_id).await;
}

async fn rdbms_execute_err_test<T: RdbmsType>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    query: &str,
    params: Vec<T::DbValue>,
    expected: Error,
) {
    let worker_id = new_worker_id();
    let connection = rdbms.create(db_address, &worker_id).await;
    check!(connection.is_ok(), "connection to {} is ok", db_address);
    let pool_key = connection.unwrap();

    let result = rdbms.execute(&pool_key, &worker_id, query, params).await;

    check!(
        result.is_err(),
        "query {} (executed on {}) - result is error",
        query,
        pool_key
    );

    let error = result.err().unwrap();
    check!(
        error == expected,
        "query {} (executed on {}) - result error match",
        query,
        pool_key
    );

    let _ = rdbms.remove(&pool_key, &worker_id).await;
}

#[test]
fn test_rdbms_pool_key_masked_address() {
    let key = RdbmsPoolKey::from("mysql://user:password@localhost:3306").unwrap();
    check!(key.masked_address() == "mysql://user:*****@localhost:3306");
    let key = RdbmsPoolKey::from("mysql://user@localhost:3306").unwrap();
    check!(key.masked_address() == "mysql://user@localhost:3306");
    let key = RdbmsPoolKey::from("mysql://localhost:3306").unwrap();
    check!(key.masked_address() == "mysql://localhost:3306");
    let key =
        RdbmsPoolKey::from("postgres://user:password@localhost:5432?abc=xyz&def=xyz").unwrap();
    check!(key.masked_address() == "postgres://user:*****@localhost:5432?abc=xyz&def=xyz");
    let key =
        RdbmsPoolKey::from("postgres://user:password@localhost:5432?abc=xyz&secret=xyz").unwrap();
    check!(key.masked_address() == "postgres://user:*****@localhost:5432?abc=xyz&secret=*****");
}

#[test]
async fn mysql_par_test(mysql: &DockerMysqlRdb, rdbms_service: &RdbmsServiceDefault) {
    let db_address = mysql.public_connection_string();
    let rdbms = rdbms_service.mysql();
    let mut db_addresses = create_test_databases(rdbms.clone(), &db_address, 3, |db_name| {
        mysql.public_connection_string_to_db(&db_name)
    })
    .await;
    db_addresses.push(db_address);

    rdbms_par_test(
        rdbms,
        db_addresses,
        3,
        RdbmsTest::new(
            vec![StatementTest::execute_test("SELECT 1", vec![], Some(0))],
            None,
        ),
    )
    .await
}

#[test]
async fn postgres_par_test(postgres: &DockerPostgresRdb, rdbms_service: &RdbmsServiceDefault) {
    let db_address = postgres.public_connection_string();
    let rdbms = rdbms_service.postgres();
    let mut db_addresses = create_test_databases(rdbms.clone(), &db_address, 3, |db_name| {
        postgres.public_connection_string_to_db(&db_name)
    })
    .await;
    db_addresses.push(db_address);

    rdbms_par_test(
        rdbms,
        db_addresses,
        3,
        RdbmsTest::new(
            vec![StatementTest::execute_test("SELECT 1", vec![], Some(1))],
            None,
        ),
    )
    .await
}

async fn create_test_databases<T: RdbmsType + 'static>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    count: u8,
    to_db_address: impl Fn(String) -> String,
) -> Vec<String> {
    let worker_id = new_worker_id();

    let connection = rdbms.create(db_address, &worker_id).await;
    check!(connection.is_ok(), "connection to {} is ok", db_address);
    let pool_key = connection.unwrap();

    let mut values: Vec<String> = Vec::with_capacity(count as usize);

    for i in 0..count {
        let db_name = format!("test_db_{i}");

        let r = rdbms
            .execute(
                &pool_key,
                &worker_id,
                &format!("CREATE DATABASE {db_name}"),
                vec![],
            )
            .await;

        check!(r.is_ok(), "db creation {} is ok", db_name);

        let address = to_db_address(db_name);
        values.push(address);
    }

    let _ = rdbms.remove(&pool_key, &worker_id).await;

    values
}

async fn rdbms_par_test<T: RdbmsType + 'static>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_addresses: Vec<String>,
    count: u8,
    test: RdbmsTest<T>,
) {
    let mut fibers = JoinSet::new();
    for db_address in &db_addresses {
        for _ in 0..count {
            let rdbms_clone = rdbms.clone();
            let db_address_clone = db_address.clone();
            let test_clone = test.clone();
            let _ = fibers.spawn(
                async move {
                    let worker_id = new_worker_id();

                    let connection = rdbms_clone.create(&db_address_clone, &worker_id).await;

                    let pool_key = connection.unwrap();

                    let (transaction_id, result) =
                        execute_rdbms_test(rdbms_clone.clone(), &pool_key, &worker_id, test_clone)
                            .await;

                    (worker_id, pool_key, transaction_id, result)
                }
                .in_current_span(),
            );
        }
    }

    let mut workers_results: HashMap<WorkerId, Vec<Result<StatementResult<T>, Error>>> =
        HashMap::new();
    let mut workers_pools: HashMap<WorkerId, RdbmsPoolKey> = HashMap::new();
    let mut workers_transactions: HashMap<WorkerId, Option<TransactionId>> = HashMap::new();

    while let Some(res) = fibers.join_next().await {
        let (worker_id, pool_key, transaction_id, result_execute) = res.unwrap();
        workers_results.insert(worker_id.clone(), result_execute);
        workers_pools.insert(worker_id.clone(), pool_key);
        workers_transactions.insert(worker_id.clone(), transaction_id);
    }

    if test.transaction_end.is_some() {
        for (worker_id, transaction_id) in workers_transactions.clone() {
            let pool_key = workers_pools.get(&worker_id).unwrap().clone();
            check_transaction(
                rdbms.clone(),
                &pool_key,
                &worker_id,
                test.transaction_end.clone(),
                transaction_id,
            )
            .await;
        }
    }

    let rdbms_status = rdbms.status().await;

    for (worker_id, pool_key) in workers_pools.clone() {
        let _ = rdbms.remove(&pool_key, &worker_id).await;
    }

    check!(rdbms_status.pools.len() == db_addresses.len());

    for (worker_id, pool_key) in workers_pools {
        let worker_ids = rdbms_status.pools.get(&pool_key);

        check!(
            worker_ids.is_some_and(|ids| ids.contains(&worker_id)),
            "worker {worker_id} found in pool {pool_key}"
        );
    }

    for (worker_id, result) in workers_results {
        check_test_results(&worker_id, test.clone(), result);
    }
}

fn new_worker_id() -> WorkerId {
    WorkerId {
        component_id: ComponentId::new_v4(),
        worker_name: "test".to_string(),
    }
}
