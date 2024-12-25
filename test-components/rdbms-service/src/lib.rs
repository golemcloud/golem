mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::rdbms::postgres::{DbConnection as PostgresDbConnection};
use crate::bindings::wasi::rdbms::postgres::{DbValue as PostgresDbValue, LazyDbValue as PostgresLazyDbValue, DbColumnType as PostgresDbColumnType, DbColumn as PostgresDbColumn, DbRow as PostgresDbRow, DbResult as PostgresDbResult};
use crate::bindings::wasi::rdbms::mysql::{DbConnection as MysqlDbConnection};
use crate::bindings::wasi::rdbms::mysql::{DbValue as MysqlDbValue, DbRow as MysqlDbRow, DbResult as MysqlDbResult};

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

    fn postgres_executions(statements: Vec<Statement>) -> Result<Vec<Result<PostgresResult, String>>, String> {
        let address = get_postgres_db_url()?;
        let connection = PostgresDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("postgres - address: {}, statements: {}", address, statements.len());

        let mut results: Vec<Result<PostgresResult, String>> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement.params.into_iter().map(get_postgres_db_param).collect::<Vec<PostgresDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let result = connection.execute(&statement.statement, params).map_err(|e| e.to_string());
                    results.push(result.map(PostgresResult::Execute));
                }
                StatementAction::Query => {
                    let result: Result<PostgresDbResult, String> = {
                        let result = connection.query(&statement.statement, params).map_err(|e| e.to_string())?;
                        let columns = result.columns.into_iter().map(get_postgres_db_column).collect();
                        let rows = result.rows.into_iter().map(get_postgres_db_row).collect();
                        Ok(PostgresDbResult { columns, rows })
                    };
                    results.push(result.map(PostgresResult::Query));
                }
                StatementAction::QueryStream => {
                    let result: Result<PostgresDbResult, String> = {
                        let result_set = connection.query_stream(&statement.statement, params).map_err(|e| e.to_string())?;
                        let columns = result_set.get_columns().into_iter().map(get_postgres_db_column).collect();
                        let mut rows: Vec<PostgresDbRow> = vec![];

                        while let Some(vs) = result_set.get_next() {
                            let rs: Vec<PostgresDbRow> = vs.into_iter().map(get_postgres_db_row).collect();
                            rows.extend(rs);
                        }
                        Ok(PostgresDbResult { columns, rows })
                    };
                    results.push(result.map(PostgresResult::Query));
                }
            }
        }
        Ok(results)
    }
    
    fn postgres_transaction(statements: Vec<Statement>, end: TransactionEnd) -> Result<Vec<Result<PostgresResult, String>>, String> {
        let address = get_postgres_db_url()?;
        let connection = PostgresDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("postgres transaction - address: {}, statements: {}, end: {:?}", address, statements.len(), end);

        let transaction = connection.begin_transaction().map_err(|e| e.to_string())?;
        let mut results: Vec<Result<PostgresResult, String>> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement.params.into_iter().map(get_postgres_db_param).collect::<Vec<PostgresDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let result = transaction.execute(&statement.statement, params).map_err(|e| e.to_string());
                    results.push(result.map(PostgresResult::Execute));
                }
                StatementAction::Query => {
                    let result: Result<PostgresDbResult, String> = {
                        let result = transaction.query(&statement.statement, params).map_err(|e| e.to_string())?;
                        let columns = result.columns.into_iter().map(get_postgres_db_column).collect();
                        let rows = result.rows.into_iter().map(get_postgres_db_row).collect();
                        Ok(PostgresDbResult { columns, rows })
                    };
                    results.push(result.map(PostgresResult::Query));
                }
                StatementAction::QueryStream => {
                    results.push(Err("Query Stream is not supported for transactions".to_string()));
                }
            }
        }

        match end {
            TransactionEnd::Commit => transaction.commit().map_err(|e| e.to_string())?,
            TransactionEnd::Rollback => transaction.rollback().map_err(|e| e.to_string())?,
            TransactionEnd::None => (), // no transaction action, drop of transaction resource should do rollback
        }

        Ok(results)
    }

    fn mysql_executions(statements: Vec<Statement>) -> Result<Vec<Result<MysqlResult, String>>, String> {
        let address = get_mysql_db_url().unwrap_or("N/A".to_string());
        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("mysql - address: {}, statements: {}", address, statements.len());

        let mut results: Vec<Result<MysqlResult, String>> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement.params.into_iter().map(MysqlDbValue::Text).collect::<Vec<MysqlDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let result = connection.execute(&statement.statement, &params).map_err(|e| e.to_string());
                    results.push(result.map(MysqlResult::Execute));
                }
                StatementAction::Query => {
                    let result = connection.query(&statement.statement, &params).map_err(|e| e.to_string());
                    results.push(result.map(MysqlResult::Query));
                }
                StatementAction::QueryStream => {
                    let result: Result<MysqlDbResult, String> = {
                        let result_set = connection.query_stream(&statement.statement, &params).map_err(|e| e.to_string())?;
                        let columns = result_set.get_columns();
                        let mut rows: Vec<MysqlDbRow> = vec![];

                        while let Some(rs) = result_set.get_next() {
                            rows.extend(rs);
                        }
                        Ok(MysqlDbResult { columns, rows })
                    };
                    results.push(result.map(MysqlResult::Query));
                }
            }
        }


        Ok(results)
    }

    fn mysql_transaction(statements: Vec<Statement>, end: TransactionEnd) -> Result<Vec<Result<MysqlResult, String>>, String> {
        let address = get_mysql_db_url().unwrap_or("N/A".to_string());
        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("mysql transaction - address: {}, statements: {}, end: {:?}", address, statements.len(), end);

        let transaction = connection.begin_transaction().map_err(|e| e.to_string())?;
        let mut results: Vec<Result<MysqlResult, String>> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement.params.into_iter().map(MysqlDbValue::Text).collect::<Vec<MysqlDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let result = transaction.execute(&statement.statement, &params).map_err(|e| e.to_string());
                    results.push(result.map(MysqlResult::Execute));
                }
                StatementAction::Query => {
                    let result = transaction.query(&statement.statement, &params).map_err(|e| e.to_string());
                    results.push(result.map(MysqlResult::Query));
                }
                StatementAction::QueryStream => {
                    results.push(Err("Query Stream is not supported for transactions".to_string()));
                }
            }
        }

        match end {
            TransactionEnd::Commit => transaction.commit().map_err(|e| e.to_string())?,
            TransactionEnd::Rollback => transaction.rollback().map_err(|e| e.to_string())?,
            TransactionEnd::None => (), // no transaction action, drop of transaction resource should do rollback
        }

        Ok(results)
    }
}

bindings::export!(Component with_types_in bindings);
