mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::rdbms::postgres::{DbConnection as PostgresDbConnection};
use crate::bindings::wasi::rdbms::postgres::{DbValue as PostgresDbValue, LazyDbValue as PostgresLazyDbValue, DbColumnType as PostgresDbColumnType, DbColumn as PostgresDbColumn, DbRow as PostgresDbRow};
use crate::bindings::wasi::rdbms::mysql::{DbConnection as MysqlDbConnection};
use crate::bindings::wasi::rdbms::mysql::{DbValue as MysqlDbValue, DbRow as MysqlDbRow};

fn get_mysql_db_url() -> Result<String, String> {
    std::env::var("DB_MYSQL_URL").map_err(|_| "DB_MYSQL_URL is not set".to_string())
}

fn get_postgres_db_url() -> Result<String, String> {
    std::env::var("DB_POSTGRES_URL").map_err(|_| "DB_POSTGRES_URL is not set".to_string())
}

fn get_postgres_db_column(value: PostgresDbColumn) -> PostgresDbColumn {
    PostgresDbColumn {
        ordinal: value.ordinal,
        name: value.name,
        db_type: get_postgres_db_column_type(value.db_type),
        db_type_name: value.db_type_name,
    }
}

fn get_postgres_db_column_type(value: PostgresDbColumnType) -> PostgresDbColumnType {
    if let PostgresDbColumnType::Array(v) = value {
        v.get()
    } else {
        value
    }
}

fn get_postgres_db_value(value: PostgresDbValue) -> PostgresDbValue {
    if let PostgresDbValue::Array(es) = value {
        let mut elements: Vec<String> = Vec::with_capacity(es.len());
        for e in es {
            let v = e.get();
            if let PostgresDbValue::Text(v) = v {
                elements.push(v);
            }
            else {
                elements.push(format!("{:?}", v));
            }
        }
        PostgresDbValue::Text(format!("[{}]", elements.join(", ")))
    } else {
        value
    }
}

fn get_postgres_db_row(row: PostgresDbRow) -> PostgresDbRow {
    let mut values: Vec<PostgresDbValue> = Vec::with_capacity(row.values.len());
    for v in row.values {
        let value = get_postgres_db_value(v);
        values.push(value);
    }
    PostgresDbRow {
        values
    }
}

fn get_postgres_db_param(value: String) -> PostgresDbValue {
    if value.starts_with('[') && value.ends_with(']') {
        let mut elements: Vec<PostgresLazyDbValue> = vec![];
        let vs = value.trim_start_matches('[').trim_end_matches(']').split(',');
        for v in vs {
            let element = PostgresLazyDbValue::create(PostgresDbValue::Text(v.trim().to_string()));
            elements.push(element)
        }
        PostgresDbValue::Array(elements)
    }
    else {
        PostgresDbValue::Text(value)
    }
}

struct Component;


impl Guest for Component {
    fn check() -> String {
        let mysql_address = get_mysql_db_url().unwrap_or("N/A".to_string());
        let postgres_address = get_postgres_db_url().unwrap_or("N/A".to_string());
        println!("check - postgres address: {}, mysql address: {}", postgres_address, mysql_address);
        "Ok".to_string()
    }

    fn postgres_execute(statement: String, params: Vec<String>) -> Result<u64, String> {
        let address = get_postgres_db_url()?;

        let connection = PostgresDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("postgres execute - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.into_iter().map(get_postgres_db_param).collect::<Vec<PostgresDbValue>>();
        let result = connection.execute(&statement, params).map_err(|e| e.to_string());

        println!("postgres execute result: {:?}", result);

        result
    }

    fn postgres_query(statement: String, params: Vec<String>) -> Result<PostgresQueryResult, String> {
        let address = get_postgres_db_url()?;

        let connection = PostgresDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("postgres query - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.into_iter().map(get_postgres_db_param).collect::<Vec<PostgresDbValue>>();
        let result_set = connection.query(&statement, params).map_err(|e| e.to_string())?;

        let columns = result_set.get_columns().into_iter().map(get_postgres_db_column).collect();
        let mut rows: Vec<PostgresDbRow> = vec![];

        while let Some(vs) = result_set.get_next() {
            let mut rs: Vec<PostgresDbRow> = Vec::with_capacity(vs.len());

            for v in vs {
                rs.push(get_postgres_db_row(v));
            }

            rows.extend(rs);
        }

        println!("postgres query result: {:?}", rows);

        Ok(PostgresQueryResult { columns, rows })
    }


    fn mysql_execute(statement: String, params: Vec<String>) -> Result<u64, String> {
        let address = get_mysql_db_url()?;

        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("mysql execute - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.into_iter().map(MysqlDbValue::Text).collect::<Vec<MysqlDbValue>>();

        let result = connection.execute(&statement, &params).map_err(|e| e.to_string());

        println!("mysql execute result: {:?}", result);

        result
    }

    fn mysql_query(statement: String, params: Vec<String>) -> Result<MysqlQueryResult, String> {
        let address = get_mysql_db_url()?;

        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("mysql query - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.into_iter().map(MysqlDbValue::Text).collect::<Vec<MysqlDbValue>>();

        let result_set = connection.query(&statement, &params).map_err(|e| e.to_string())?;
        let columns = result_set.get_columns();
        let mut rows: Vec<MysqlDbRow> = vec![];

        while let Some(rs) = result_set.get_next() {
            rows.extend(rs);
        }

        println!("mysql query result: {:?}", rows);

        Ok(MysqlQueryResult { columns, rows })
    }
}

bindings::export!(Component with_types_in bindings);
