mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::rdbms::postgres::{DbConnection as PostgresDbConnection};
use crate::bindings::wasi::rdbms::postgres::{DbValue as PostgresDbValue, DbValuePrimitive as PostgresDbValuePrimitive, DbRow as PostgresDbRow};
use crate::bindings::wasi::rdbms::mysql::{DbConnection as MysqlDbConnection};
use crate::bindings::wasi::rdbms::mysql::{DbValue as MysqlDbValue, DbRow as MysqlDbRow};

fn get_mysql_db_url() -> Result<String, String> {
    std::env::var("DB_MYSQL_URL").map_err(|_| "DB_MYSQL_URL is not set".to_string())
}

fn get_postgres_db_url() -> Result<String, String> {
    std::env::var("DB_POSTGRES_URL").map_err(|_| "DB_POSTGRES_URL is not set".to_string())
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

        let params = params.iter().map(|v| PostgresDbValue::Primitive(PostgresDbValuePrimitive::Text(v.clone()))).collect::<Vec<PostgresDbValue>>();

        let result = connection.execute(&statement, &params).map_err(|e| e.to_string());

        println!("postgres execute result: {:?}", result);

        result
    }

    fn postgres_query(statement: String, params: Vec<String>) -> Result<PostgresQueryResult, String> {
        let address = get_postgres_db_url()?;

        let connection = PostgresDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("postgres query - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.iter().map(|v| PostgresDbValue::Primitive(PostgresDbValuePrimitive::Text(v.clone()))).collect::<Vec<PostgresDbValue>>();

        let result_set = connection.query(&statement, &params).map_err(|e| e.to_string())?;

        let columns = result_set.get_columns();
        let mut rows: Vec<PostgresDbRow> = vec![];
        loop {
            match result_set.get_next() {
                Some(values) => {
                    rows.extend(values);
                }
                None => break,
            }
        }

        println!("postgres query result: {:?}", rows);

        Ok(PostgresQueryResult { columns, rows })
    }


    fn mysql_execute(statement: String, params: Vec<String>) -> Result<u64, String> {
        let address = get_mysql_db_url()?;

        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("mysql execute - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.iter().map(|v| MysqlDbValue::Text(v.clone())).collect::<Vec<MysqlDbValue>>();

        let result = connection.execute(&statement, &params).map_err(|e| e.to_string());

        println!("mysql execute result: {:?}", result);

        result
    }

    fn mysql_query(statement: String, params: Vec<String>) -> Result<MysqlQueryResult, String> {
        let address = get_mysql_db_url()?;

        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("mysql query - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.iter().map(|v| MysqlDbValue::Text(v.clone())).collect::<Vec<MysqlDbValue>>();

        let result_set = connection.query(&statement, &params).map_err(|e| e.to_string())?;
        let columns = result_set.get_columns();
        let mut rows: Vec<MysqlDbRow> = vec![];
        loop {
            match result_set.get_next() {
                Some(values) => {
                    rows.extend(values);
                }
                None => break,
            }
        }

        println!("mysql query result: {:?}", rows);

        Ok(MysqlQueryResult { columns, rows })
    }
}

bindings::export!(Component with_types_in bindings);
