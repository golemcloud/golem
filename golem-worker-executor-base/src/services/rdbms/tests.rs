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

use crate::services::rdbms::mysql::types as mysql_types;
use crate::services::rdbms::postgres::types as postgres_types;
use crate::services::rdbms::{DbRow, Error};
use crate::services::rdbms::{Rdbms, RdbmsServiceDefault, RdbmsType};
use crate::services::rdbms::{RdbmsPoolKey, RdbmsService};
use assert2::check;
use bigdecimal::BigDecimal;
use chrono::Offset;
use golem_common::model::{ComponentId, WorkerId};
use golem_test_framework::components::rdb::docker_mysql::DockerMysqlRdbs;
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdbs;
use golem_test_framework::components::rdb::{RdbConnection, RdbsConnections};
use serde_json::json;
use sqlx::types::mac_address::MacAddress;
use sqlx::types::BitVec;
use std::collections::{Bound, HashMap};
use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
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
async fn postgres_execute_test_create_insert_select(
    postgres: &DockerPostgresRdbs,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.rdbs[1].host_connection_string();
    let rdbms = rdbms_service.postgres();

    let create_enum_statement = r#"
            CREATE TYPE test_enum AS ENUM ('regular', 'special');
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_enum_statement,
        vec![],
        None,
    )
    .await;

    let create_custom_type_statement = r#"
            CREATE TYPE inventory_item AS (
                product_id      uuid,
                name            text,
                supplier_id     INT4,
                price           numeric
            );
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_custom_type_statement,
        vec![],
        None,
    )
    .await;

    let create_table_statement = r#"
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
                inventory_item_col inventory_item
            );
        "#;

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
            inventory_item_col
            )
            VALUES
            (
                $1, $2::test_enum, $3, $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21, $22, $23, $24, $25, $26, $27, $28, $29,
                $30, $31, $32, $33, $34, $35, $36, $37, $38::tsvector, $39::tsquery, $40
            );
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_table_statement,
        vec![],
        None,
    )
    .await;

    let count = 4;

    let mut rows: Vec<DbRow<postgres_types::DbValue>> = Vec::with_capacity(count);

    for i in 0..count {
        let mut params: Vec<postgres_types::DbValue> = vec![postgres_types::DbValue::Primitive(
            postgres_types::DbValuePrimitive::Varchar(format!("{:03}", i)),
        )];

        if i % 2 == 0 {
            let tstzbounds = (
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
            let tsbounds = (
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
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::CustomEnum(
                    "regular".to_string(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Character(2)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Int2(1)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Int4(2)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Int8(3)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Float4(4.0)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Float8(5.0)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Numeric(
                    BigDecimal::from(48888),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Boolean(true)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Text(
                    "text".to_string(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Varchar(
                    "varchar".to_string(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Bpchar(
                    "0123456789".to_string(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Timestamp(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Timestamptz(
                    chrono::DateTime::from_naive_utc_and_offset(
                        chrono::NaiveDateTime::new(
                            chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                            chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                        ),
                        chrono::Utc,
                    ),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Date(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Time(
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Timetz((
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    chrono::Utc.fix(),
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Interval((
                    10, 20, 30,
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Bytea(
                    "bytea".as_bytes().to_vec(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Uuid(
                    Uuid::new_v4(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Json(json!(
                       {
                          "id": i
                       }
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Jsonb(json!(
                       {
                          "index": i
                       }
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Inet(
                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Cidr(
                    IpAddr::V4(Ipv4Addr::new(198, 168, 0, i as u8)),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Macaddr(
                    MacAddress::new([0, 1, 2, 3, 4, i as u8]),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Bit(
                    BitVec::from_iter(vec![true, false, true]),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Varbit(
                    BitVec::from_iter(vec![true, false, false]),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Xml(format!(
                    "<foo>{}</foo>",
                    i
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Int4range((
                    Bound::Included(1),
                    Bound::Excluded(4),
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Int8range((
                    Bound::Included(1),
                    Bound::Unbounded,
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Numrange((
                    Bound::Included(BigDecimal::from(11)),
                    Bound::Excluded(BigDecimal::from(221)),
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Tsrange(
                    tsbounds,
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Tstzrange(
                    tstzbounds,
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Daterange((
                    Bound::Included(
                        chrono::NaiveDate::from_ymd_opt(2023 + i as i32, 2, 3).unwrap(),
                    ),
                    Bound::Unbounded,
                ))),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Money(12345)),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Jsonpath(
                    "$.id".to_string(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Text(
                    "'a' 'and' 'ate' 'cat' 'fat' 'mat' 'on' 'rat' 'sat'".to_string(),
                )),
                postgres_types::DbValue::Primitive(postgres_types::DbValuePrimitive::Text(
                    "'fat' & 'rat' & !'cat'".to_string(),
                )),
                postgres_types::DbValue::Primitive(
                    postgres_types::DbValuePrimitive::CustomComposite((
                        "inventory_item".to_string(),
                        vec![
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Uuid(Uuid::new_v4()),
                            ),
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Text("text".to_string()),
                            ),
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Int4(i as i32),
                            ),
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Numeric(BigDecimal::from(111)),
                            ),
                        ],
                    )),
                ),
            ]);
        } else {
            for _ in 0..39 {
                params.push(postgres_types::DbValue::Primitive(
                    postgres_types::DbValuePrimitive::Null,
                ));
            }
        }

        rdbms_execute_test(
            rdbms.clone(),
            &db_address,
            insert_statement,
            params.clone(),
            Some(1),
        )
        .await;

        rows.push(DbRow { values: params });
    }

    let expected_columns = vec![
        postgres_types::DbColumn {
            name: "id".to_string(),
            ordinal: 0,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Varchar,
            ),
            db_type_name: "VARCHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "enum_col".to_string(),
            ordinal: 1,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::CustomEnum("test_enum".to_string()),
            ),
            db_type_name: "test_enum".to_string(),
        },
        postgres_types::DbColumn {
            name: "char_col".to_string(),
            ordinal: 2,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Character,
            ),
            db_type_name: "\"CHAR\"".to_string(),
        },
        postgres_types::DbColumn {
            name: "int2_col".to_string(),
            ordinal: 3,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Int2,
            ),
            db_type_name: "INT2".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4_col".to_string(),
            ordinal: 4,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Int4,
            ),
            db_type_name: "INT4".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8_col".to_string(),
            ordinal: 5,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Int8,
            ),
            db_type_name: "INT8".to_string(),
        },
        postgres_types::DbColumn {
            name: "float4_col".to_string(),
            ordinal: 6,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Float4,
            ),
            db_type_name: "FLOAT4".to_string(),
        },
        postgres_types::DbColumn {
            name: "float8_col".to_string(),
            ordinal: 7,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Float8,
            ),
            db_type_name: "FLOAT8".to_string(),
        },
        postgres_types::DbColumn {
            name: "numeric_col".to_string(),
            ordinal: 8,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Numeric,
            ),
            db_type_name: "NUMERIC".to_string(),
        },
        postgres_types::DbColumn {
            name: "boolean_col".to_string(),
            ordinal: 9,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Boolean,
            ),
            db_type_name: "BOOL".to_string(),
        },
        postgres_types::DbColumn {
            name: "text_col".to_string(),
            ordinal: 10,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Text,
            ),
            db_type_name: "TEXT".to_string(),
        },
        postgres_types::DbColumn {
            name: "varchar_col".to_string(),
            ordinal: 11,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Varchar,
            ),
            db_type_name: "VARCHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "bpchar_col".to_string(),
            ordinal: 12,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Bpchar,
            ),
            db_type_name: "CHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamp_col".to_string(),
            ordinal: 13,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Timestamp,
            ),
            db_type_name: "TIMESTAMP".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamptz_col".to_string(),
            ordinal: 14,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Timestamptz,
            ),
            db_type_name: "TIMESTAMPTZ".to_string(),
        },
        postgres_types::DbColumn {
            name: "date_col".to_string(),
            ordinal: 15,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Date,
            ),
            db_type_name: "DATE".to_string(),
        },
        postgres_types::DbColumn {
            name: "time_col".to_string(),
            ordinal: 16,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Time,
            ),
            db_type_name: "TIME".to_string(),
        },
        postgres_types::DbColumn {
            name: "timetz_col".to_string(),
            ordinal: 17,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Timetz,
            ),
            db_type_name: "TIMETZ".to_string(),
        },
        postgres_types::DbColumn {
            name: "interval_col".to_string(),
            ordinal: 18,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Interval,
            ),
            db_type_name: "INTERVAL".to_string(),
        },
        postgres_types::DbColumn {
            name: "bytea_col".to_string(),
            ordinal: 19,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Bytea,
            ),
            db_type_name: "BYTEA".to_string(),
        },
        postgres_types::DbColumn {
            name: "uuid_col".to_string(),
            ordinal: 20,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Uuid,
            ),
            db_type_name: "UUID".to_string(),
        },
        postgres_types::DbColumn {
            name: "json_col".to_string(),
            ordinal: 21,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Json,
            ),
            db_type_name: "JSON".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonb_col".to_string(),
            ordinal: 22,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Jsonb,
            ),
            db_type_name: "JSONB".to_string(),
        },
        postgres_types::DbColumn {
            name: "inet_col".to_string(),
            ordinal: 23,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Inet,
            ),
            db_type_name: "INET".to_string(),
        },
        postgres_types::DbColumn {
            name: "cidr_col".to_string(),
            ordinal: 24,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Cidr,
            ),
            db_type_name: "CIDR".to_string(),
        },
        postgres_types::DbColumn {
            name: "macaddr_col".to_string(),
            ordinal: 25,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Macaddr,
            ),
            db_type_name: "MACADDR".to_string(),
        },
        postgres_types::DbColumn {
            name: "bit_col".to_string(),
            ordinal: 26,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Bit,
            ),
            db_type_name: "BIT".to_string(),
        },
        postgres_types::DbColumn {
            name: "varbit_col".to_string(),
            ordinal: 27,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Varbit,
            ),
            db_type_name: "VARBIT".to_string(),
        },
        postgres_types::DbColumn {
            name: "xml_col".to_string(),
            ordinal: 28,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Xml,
            ),
            db_type_name: "XML".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4range_col".to_string(),
            ordinal: 29,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Int4range,
            ),
            db_type_name: "INT4RANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8range_col".to_string(),
            ordinal: 30,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Int8range,
            ),
            db_type_name: "INT8RANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "numrange_col".to_string(),
            ordinal: 31,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Numrange,
            ),
            db_type_name: "NUMRANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsrange_col".to_string(),
            ordinal: 32,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Tsrange,
            ),
            db_type_name: "TSRANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "tstzrange_col".to_string(),
            ordinal: 33,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Tstzrange,
            ),
            db_type_name: "TSTZRANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "daterange_col".to_string(),
            ordinal: 34,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Daterange,
            ),
            db_type_name: "DATERANGE".to_string(),
        },
        postgres_types::DbColumn {
            name: "money_col".to_string(),
            ordinal: 35,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Money,
            ),
            db_type_name: "MONEY".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonpath_col".to_string(),
            ordinal: 36,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Jsonpath,
            ),
            db_type_name: "JSONPATH".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsvector_col".to_string(),
            ordinal: 37,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Text,
            ),
            db_type_name: "TEXT".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsquery_col".to_string(),
            ordinal: 38,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Text,
            ),
            db_type_name: "TEXT".to_string(),
        },
        postgres_types::DbColumn {
            name: "inventory_item_col".to_string(),
            ordinal: 39,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::CustomComposite((
                    "inventory_item".to_string(),
                    vec![
                        (
                            "product_id".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Uuid,
                            ),
                        ),
                        (
                            "name".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Text,
                            ),
                        ),
                        (
                            "supplier_id".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Int4,
                            ),
                        ),
                        (
                            "price".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Numeric,
                            ),
                        ),
                    ],
                )),
            ),
            db_type_name: "inventory_item".to_string(),
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
            inventory_item_col
           FROM data_types ORDER BY id ASC;
        "#;

    rdbms_query_test(
        rdbms.clone(),
        &db_address,
        select_statement,
        vec![],
        Some(expected_columns),
        Some(rows),
    )
    .await;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        "DELETE FROM data_types;",
        vec![],
        None,
    )
    .await;
}

#[test]
async fn postgres_execute_test_create_insert_select_array(
    postgres: &DockerPostgresRdbs,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.rdbs[1].host_connection_string();
    let rdbms = rdbms_service.postgres();

    let create_enum_statement = r#"
            CREATE TYPE a_test_enum AS ENUM ('first', 'second', 'third');
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_enum_statement,
        vec![],
        None,
    )
    .await;

    let create_custom_type_statement = r#"
            CREATE TYPE a_inventory_item AS (
                product_id      uuid,
                name            text,
                supplier_id     INT4,
                price           numeric
            );
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_custom_type_statement,
        vec![],
        None,
    )
    .await;

    let create_table_statement = r#"
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
                inventory_item_col a_inventory_item[]
            );
        "#;

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
            inventory_item_col
            )
            VALUES
            (
                $1, $2::a_test_enum[], $3, $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $15, $16, $17, $18, $19,
                $20, $21, $22, $23, $24, $25, $26, $27, $28, $29,
                $30, $31, $32, $33, $34, $35, $36, $37, $38::tsvector[], $39::tsquery[], $40
            );
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_table_statement,
        vec![],
        None,
    )
    .await;

    let count = 4;

    let mut rows: Vec<DbRow<postgres_types::DbValue>> = Vec::with_capacity(count);

    for i in 0..count {
        let mut params: Vec<postgres_types::DbValue> = vec![postgres_types::DbValue::Primitive(
            postgres_types::DbValuePrimitive::Varchar(format!("{:03}", i)),
        )];

        if i % 2 == 0 {
            let tstzbounds = (
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
            let tsbounds = (
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
                    postgres_types::DbValuePrimitive::CustomEnum("second".to_string()),
                    postgres_types::DbValuePrimitive::CustomEnum("third".to_string()),
                ]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Character(
                    2,
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Int2(1)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Int4(2)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Int8(3)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Float4(4.0)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Float8(5.0)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Numeric(
                    BigDecimal::from(48888),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Boolean(
                    true,
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Text(
                    "text".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Varchar(
                    "varchar".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Bpchar(
                    "0123456789".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Timestamp(
                    chrono::NaiveDateTime::new(
                        chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                        chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    ),
                )]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValuePrimitive::Timestamptz(
                        chrono::DateTime::from_naive_utc_and_offset(
                            chrono::NaiveDateTime::new(
                                chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                                chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                            ),
                            chrono::Utc,
                        ),
                    ),
                ]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Date(
                    chrono::NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Time(
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Timetz((
                    chrono::NaiveTime::from_hms_opt(10, 20, 30).unwrap(),
                    chrono::Utc.fix(),
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Interval((
                    10, 20, 30,
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Bytea(
                    "bytea".as_bytes().to_vec(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Uuid(
                    Uuid::new_v4(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Json(
                    json!(
                           {
                              "id": i
                           }
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Jsonb(
                    json!(
                           {
                              "index": i
                           }
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Inet(
                    IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Cidr(
                    IpAddr::V4(Ipv4Addr::new(198, 168, 0, i as u8)),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Macaddr(
                    MacAddress::new([0, 1, 2, 3, 4, i as u8]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Bit(
                    BitVec::from_iter(vec![true, false, true]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Varbit(
                    BitVec::from_iter(vec![true, false, false]),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Xml(
                    format!("<foo>{}</foo>", i),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Int4range(
                    (Bound::Included(1), Bound::Excluded(4)),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Int8range(
                    (Bound::Included(1), Bound::Unbounded),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Numrange((
                    Bound::Included(BigDecimal::from(11)),
                    Bound::Excluded(BigDecimal::from(221)),
                ))]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Tsrange(
                    tsbounds,
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Tstzrange(
                    tstzbounds,
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Daterange(
                    (
                        Bound::Included(
                            chrono::NaiveDate::from_ymd_opt(2023 + i as i32, 2, 3).unwrap(),
                        ),
                        Bound::Unbounded,
                    ),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Money(1234)]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Jsonpath(
                    "$.user.addresses[0].city".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Text(
                    "'a' 'and' 'ate' 'cat' 'fat' 'mat' 'on' 'rat' 'sat'".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![postgres_types::DbValuePrimitive::Text(
                    "'fat' & 'rat' & !'cat'".to_string(),
                )]),
                postgres_types::DbValue::Array(vec![
                    postgres_types::DbValuePrimitive::CustomComposite((
                        "a_inventory_item".to_string(),
                        vec![
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Uuid(Uuid::new_v4()),
                            ),
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Text("text".to_string()),
                            ),
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Int4(i as i32),
                            ),
                            postgres_types::DbValue::Primitive(
                                postgres_types::DbValuePrimitive::Numeric(BigDecimal::from(111)),
                            ),
                        ],
                    )),
                ]),
            ]);
        } else {
            for _ in 0..39 {
                params.push(postgres_types::DbValue::Array(vec![]));
            }
        }

        rdbms_execute_test(
            rdbms.clone(),
            &db_address,
            insert_statement,
            params.clone(),
            Some(1),
        )
        .await;

        rows.push(DbRow { values: params });
    }

    let expected_columns = vec![
        postgres_types::DbColumn {
            name: "id".to_string(),
            ordinal: 0,
            db_type: postgres_types::DbColumnType::Primitive(
                postgres_types::DbColumnTypePrimitive::Varchar,
            ),
            db_type_name: "VARCHAR".to_string(),
        },
        postgres_types::DbColumn {
            name: "enum_col".to_string(),
            ordinal: 1,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::CustomEnum("a_test_enum".to_string()),
            ),
            db_type_name: "a_test_enum[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "char_col".to_string(),
            ordinal: 2,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Character,
            ),
            db_type_name: "\"CHAR\"[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int2_col".to_string(),
            ordinal: 3,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Int2,
            ),
            db_type_name: "INT2[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4_col".to_string(),
            ordinal: 4,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Int4,
            ),
            db_type_name: "INT4[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8_col".to_string(),
            ordinal: 5,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Int8,
            ),
            db_type_name: "INT8[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "float4_col".to_string(),
            ordinal: 6,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Float4,
            ),
            db_type_name: "FLOAT4[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "float8_col".to_string(),
            ordinal: 7,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Float8,
            ),
            db_type_name: "FLOAT8[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "numeric_col".to_string(),
            ordinal: 8,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Numeric,
            ),
            db_type_name: "NUMERIC[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "boolean_col".to_string(),
            ordinal: 9,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Boolean,
            ),
            db_type_name: "BOOL[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "text_col".to_string(),
            ordinal: 10,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Text,
            ),
            db_type_name: "TEXT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "varchar_col".to_string(),
            ordinal: 11,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Varchar,
            ),
            db_type_name: "VARCHAR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "bpchar_col".to_string(),
            ordinal: 12,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Bpchar,
            ),
            db_type_name: "CHAR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamp_col".to_string(),
            ordinal: 13,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Timestamp,
            ),
            db_type_name: "TIMESTAMP[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "timestamptz_col".to_string(),
            ordinal: 14,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Timestamptz,
            ),
            db_type_name: "TIMESTAMPTZ[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "date_col".to_string(),
            ordinal: 15,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Date,
            ),
            db_type_name: "DATE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "time_col".to_string(),
            ordinal: 16,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Time,
            ),
            db_type_name: "TIME[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "timetz_col".to_string(),
            ordinal: 17,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Timetz,
            ),
            db_type_name: "TIMETZ[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "interval_col".to_string(),
            ordinal: 18,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Interval,
            ),
            db_type_name: "INTERVAL[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "bytea_col".to_string(),
            ordinal: 19,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Bytea,
            ),
            db_type_name: "BYTEA[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "uuid_col".to_string(),
            ordinal: 20,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Uuid,
            ),
            db_type_name: "UUID[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "json_col".to_string(),
            ordinal: 21,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Json,
            ),
            db_type_name: "JSON[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonb_col".to_string(),
            ordinal: 22,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Jsonb,
            ),
            db_type_name: "JSONB[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "inet_col".to_string(),
            ordinal: 23,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Inet,
            ),
            db_type_name: "INET[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "cidr_col".to_string(),
            ordinal: 24,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Cidr,
            ),
            db_type_name: "CIDR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "macaddr_col".to_string(),
            ordinal: 25,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Macaddr,
            ),
            db_type_name: "MACADDR[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "bit_col".to_string(),
            ordinal: 26,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Bit,
            ),
            db_type_name: "BIT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "varbit_col".to_string(),
            ordinal: 27,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Varbit,
            ),
            db_type_name: "VARBIT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "xml_col".to_string(),
            ordinal: 28,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Xml,
            ),
            db_type_name: "XML[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int4range_col".to_string(),
            ordinal: 29,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Int4range,
            ),
            db_type_name: "INT4RANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "int8range_col".to_string(),
            ordinal: 30,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Int8range,
            ),
            db_type_name: "INT8RANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "numrange_col".to_string(),
            ordinal: 31,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Numrange,
            ),
            db_type_name: "NUMRANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsrange_col".to_string(),
            ordinal: 32,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Tsrange,
            ),
            db_type_name: "TSRANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tstzrange_col".to_string(),
            ordinal: 33,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Tstzrange,
            ),
            db_type_name: "TSTZRANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "daterange_col".to_string(),
            ordinal: 34,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Daterange,
            ),
            db_type_name: "DATERANGE[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "money_col".to_string(),
            ordinal: 35,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Money,
            ),
            db_type_name: "MONEY[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "jsonpath_col".to_string(),
            ordinal: 36,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Jsonpath,
            ),
            db_type_name: "JSONPATH[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsvector_col".to_string(),
            ordinal: 37,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Text,
            ),
            db_type_name: "TEXT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "tsquery_col".to_string(),
            ordinal: 38,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::Text,
            ),
            db_type_name: "TEXT[]".to_string(),
        },
        postgres_types::DbColumn {
            name: "inventory_item_col".to_string(),
            ordinal: 39,
            db_type: postgres_types::DbColumnType::Array(
                postgres_types::DbColumnTypePrimitive::CustomComposite((
                    "a_inventory_item".to_string(),
                    vec![
                        (
                            "product_id".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Uuid,
                            ),
                        ),
                        (
                            "name".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Text,
                            ),
                        ),
                        (
                            "supplier_id".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Int4,
                            ),
                        ),
                        (
                            "price".to_string(),
                            postgres_types::DbColumnType::Primitive(
                                postgres_types::DbColumnTypePrimitive::Numeric,
                            ),
                        ),
                    ],
                )),
            ),
            db_type_name: "a_inventory_item[]".to_string(),
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
            inventory_item_col
           FROM array_data_types ORDER BY id ASC;
        "#;

    rdbms_query_test(
        rdbms.clone(),
        &db_address,
        select_statement,
        vec![],
        Some(expected_columns),
        Some(rows),
    )
    .await;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        "DELETE FROM array_data_types;",
        vec![],
        None,
    )
    .await;
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
              `bigint_unsigned_col` BIGINT UNSIGNED
            );
        "#;

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
              year_col
            ) VALUES (
              ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?, ?, ?,
              ?, ?, ?, ?, ?,
              ?
            );
        "#;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        create_table_statement,
        vec![],
        None,
    )
    .await;

    let count = 2;

    let mut rows: Vec<DbRow<mysql_types::DbValue>> = Vec::with_capacity(count);

    for i in 0..count {
        let mut params: Vec<mysql_types::DbValue> =
            vec![mysql_types::DbValue::Varchar(format!("{:03}", i))];

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
                mysql_types::DbValue::Json(json!(
                       {
                          "id": i
                       }
                )),
                mysql_types::DbValue::Bit(BitVec::from_iter([true, false, false])),
                mysql_types::DbValue::TinyintUnsigned(10),
                mysql_types::DbValue::SmallintUnsigned(20),
                mysql_types::DbValue::MediumintUnsigned(30),
                mysql_types::DbValue::IntUnsigned(40),
                mysql_types::DbValue::BigintUnsigned(50),
                mysql_types::DbValue::Year(2020),
            ]);
        } else {
            for _ in 0..33 {
                params.push(mysql_types::DbValue::Null);
            }
        };

        rdbms_execute_test(
            rdbms.clone(),
            &db_address,
            insert_statement,
            params.clone(),
            Some(1),
        )
        .await;

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
              year_col
           FROM data_types ORDER BY id ASC;
        "#;

    rdbms_query_test(
        rdbms.clone(),
        &db_address,
        select_statement,
        vec![],
        Some(expected_columns),
        Some(rows),
    )
    .await;

    rdbms_execute_test(
        rdbms.clone(),
        &db_address,
        "DELETE FROM data_types;",
        vec![],
        None,
    )
    .await;
}

async fn rdbms_execute_test<T: RdbmsType>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    query: &str,
    params: Vec<T::DbValue>,
    expected: Option<u64>,
) {
    let worker_id = new_worker_id();
    let connection = rdbms.create(db_address, &worker_id).await;
    check!(connection.is_ok(), "connection to {} failed", db_address);
    let pool_key = connection.unwrap();

    let result = rdbms.execute(&pool_key, &worker_id, query, params).await;

    if result.is_err() {
        println!("error {}", result.clone().err().unwrap());
    }

    check!(
        result.is_ok(),
        "query {} (executed on {}) failed",
        query,
        pool_key
    );
    if let Some(expected) = expected {
        check!(
            result.unwrap() == expected,
            "query {} (executed on {}) - result do not match",
            query,
            pool_key
        );
    }
    let _ = rdbms.remove(&pool_key, &worker_id);
    let exists = rdbms.exists(&pool_key, &worker_id);
    check!(!exists);
}

async fn rdbms_query_test<T: RdbmsType>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_address: &str,
    query: &str,
    params: Vec<T::DbValue>,
    expected_columns: Option<Vec<T::DbColumn>>,
    expected_rows: Option<Vec<DbRow<T::DbValue>>>,
) {
    let worker_id = new_worker_id();
    let connection = rdbms.create(db_address, &worker_id).await;
    check!(connection.is_ok(), "connection to {} failed", db_address);
    let pool_key = connection.unwrap();

    let result = rdbms.query(&pool_key, &worker_id, query, params).await;

    check!(
        result.is_ok(),
        "query {} (executed on {}) failed",
        query,
        pool_key
    );

    let result = result.unwrap();
    let columns = result.get_columns().await.unwrap();

    if let Some(expected) = expected_columns {
        println!("columns: {:#?}", columns);
        println!("expected: {:#?}", expected);
        check!(
            columns == expected,
            "query {} (executed on {}) - response columns do not match",
            query,
            pool_key
        );
    }

    let mut rows: Vec<DbRow<T::DbValue>> = vec![];

    while let Some(vs) = result.get_next().await.unwrap() {
        rows.extend(vs);
    }

    if let Some(expected) = expected_rows {
        println!("rows: {:#?}", rows);
        println!("expected: {:#?}", expected);
        check!(
            rows == expected,
            "query {} (executed on {}) - response rows do not match",
            query,
            pool_key
        );
    }

    let _ = rdbms.remove(&pool_key, &worker_id);

    let exists = rdbms.exists(&pool_key, &worker_id);
    check!(!exists);
}

async fn rdbms_par_test<T: RdbmsType + 'static>(
    rdbms: Arc<dyn Rdbms<T> + Send + Sync>,
    db_addresses: Vec<String>,
    count: u8,
    query: &'static str,
    params: Vec<T::DbValue>,
    expected: Option<u64>,
) {
    let mut fibers = JoinSet::new();
    for db_address in &db_addresses {
        for _ in 0..count {
            let rdbms_clone = rdbms.clone();
            let db_address_clone = db_address.clone();
            let params_clone = params.clone();
            let _ = fibers.spawn(async move {
                let worker_id = new_worker_id();

                let connection = rdbms_clone.create(&db_address_clone, &worker_id).await;

                let pool_key = connection.unwrap();

                let result: Result<u64, Error> = rdbms_clone
                    .execute(&pool_key, &worker_id, query, params_clone)
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
        let _ = rdbms.remove(&pool_key, &worker_id);
    }

    check!(rdbms_status.pools.len() == db_addresses.len());

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
            check!(
                result.unwrap() == expected,
                "result for worker {worker_id} do not match"
            );
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
    postgres: &DockerPostgresRdbs,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.rdbs[1].host_connection_string();
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
    // rdbms_query_err_test(
    //     rdbms.clone(),
    //     &db_address,
    //     "SELECT '12.34'::float8::numeric::money;",
    //     vec![],
    //     Error::QueryResponseFailure("Column type 'MONEY' is not supported".to_string()),
    // )
    // .await;

    rdbms_query_err_test(
        rdbms.clone(),
        &db_address,
        "SELECT 1",
        vec![postgres_types::DbValue::Array(vec![
            postgres_types::DbValuePrimitive::Text("tag1".to_string()),
            postgres_types::DbValuePrimitive::Int8(0),
        ])],
        Error::QueryParameterFailure(
            "Array element '0' with index 1 has different type than expected".to_string(),
        ),
    )
    .await;
}

#[test]
async fn postgres_execute_err_test(
    postgres: &DockerPostgresRdbs,
    rdbms_service: &RdbmsServiceDefault,
) {
    let db_address = postgres.rdbs[1].host_connection_string();
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
            postgres_types::DbValuePrimitive::Text("tag1".to_string()),
            postgres_types::DbValuePrimitive::Int8(0),
        ])],
        Error::QueryParameterFailure(
            "Array element '0' with index 1 has different type than expected".to_string(),
        ),
    )
    .await;
}

#[test]
async fn mysql_query_err_test(mysql: &DockerMysqlRdbs, rdbms_service: &RdbmsServiceDefault) {
    let db_address = mysql.rdbs[1].clone().host_connection_string();
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

    // rdbms_query_err_test(
    //     rdbms.clone(),
    //     &db_address,
    //     "SELECT YEAR(\"2017-06-15\");",
    //     vec![],
    //     Error::QueryResponseFailure("Value type 'YEAR' is not supported".to_string()),
    // )
    // .await;
}

#[test]
async fn mysql_execute_err_test(mysql: &DockerMysqlRdbs, rdbms_service: &RdbmsServiceDefault) {
    let db_address = mysql.rdbs[1].clone().host_connection_string();
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

    check!(result.is_err(), "connection to {db_address} is not error");

    let error = result.err().unwrap();
    // println!("error: {}", error);
    check!(
        error == expected,
        "connection error for {db_address} do not match"
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
    check!(connection.is_ok(), "connection to {} failed", db_address);
    let pool_key = connection.unwrap();

    let result = rdbms.query(&pool_key, &worker_id, query, params).await;

    check!(
        result.is_err(),
        "query {} (executed on {}) - response is not error",
        query,
        pool_key
    );

    let error = result.err().unwrap();
    // println!("error: {}", error);
    check!(
        error == expected,
        "query {} (executed on {}) - response error do not match",
        query,
        pool_key
    );

    let _ = rdbms.remove(&pool_key, &worker_id);
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
    check!(connection.is_ok(), "connection to {} failed", db_address);
    let pool_key = connection.unwrap();

    let result = rdbms.execute(&pool_key, &worker_id, query, params).await;

    check!(
        result.is_err(),
        "query {} (executed on {}) - response is not error",
        query,
        pool_key
    );

    let error = result.err().unwrap();
    // println!("error: {}", error);
    check!(
        error == expected,
        "query {} (executed on {}) - response error do not match",
        query,
        pool_key
    );

    let _ = rdbms.remove(&pool_key, &worker_id);
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

fn new_worker_id() -> WorkerId {
    WorkerId {
        component_id: ComponentId::new_v4(),
        worker_name: "test".to_string(),
    }
}
