use golem_rust::bindings::golem::rdbms::ignite2::{
    DbConnection as IgniteDbConnection, DbValue as IgniteDbValue,
};
use golem_rust::bindings::golem::rdbms::mysql::{
    DbConnection as MysqlDbConnection, DbValue as MysqlDbValue,
};
use golem_rust::bindings::golem::rdbms::postgres::{
    DbColumnType as PostgresDbColumnType, DbConnection as PostgresDbConnection,
    DbRow as PostgresDbRow, DbValue as PostgresDbValue, LazyDbValue as PostgresLazyDbValue,
};
use golem_rust::{FromSchema, IntoSchema, agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, IntoSchema, FromSchema, Serialize, Deserialize)]
pub struct Statement {
    pub statement: String,
    pub params: Vec<String>,
    pub action: StatementAction,
    pub sleep: Option<u64>,
}

#[derive(Clone, Debug, IntoSchema, FromSchema, Serialize, Deserialize)]
pub enum StatementAction {
    Execute,
    Query,
    QueryStream,
}

#[derive(Clone, Debug, IntoSchema, FromSchema, Serialize, Deserialize)]
pub enum TransactionEnd {
    Commit,
    Rollback,
    None,
}

#[agent_definition]
pub trait RelationalDatabases {
    fn new(name: String) -> Self;
    fn check(&self) -> String;
    fn postgres_executions(&self, statements: Vec<Statement>) -> Result<String, String>;
    fn postgres_transaction(
        &self,
        statements: Vec<Statement>,
        end: TransactionEnd,
    ) -> Result<String, String>;
    fn mysql_executions(&self, statements: Vec<Statement>) -> Result<String, String>;
    fn mysql_transaction(
        &self,
        statements: Vec<Statement>,
        end: TransactionEnd,
    ) -> Result<String, String>;
    fn postgres_operation_result(
        &self,
        operation: String,
        address: String,
        statement: String,
    ) -> Result<String, String>;
    fn mysql_operation_result(
        &self,
        operation: String,
        address: String,
        statement: String,
    ) -> Result<String, String>;
    fn ignite_operation_result(
        &self,
        operation: String,
        address: String,
        statement: String,
    ) -> Result<String, String>;
}

pub struct RelationalDatabasesImpl {
    _name: String,
}

fn get_mysql_db_url() -> Result<String, String> {
    std::env::var("DB_MYSQL_URL").map_err(|_| "DB_MYSQL_URL is not set".to_string())
}

fn get_postgres_db_url() -> Result<String, String> {
    std::env::var("DB_POSTGRES_URL").map_err(|_| "DB_POSTGRES_URL is not set".to_string())
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
            } else {
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
    PostgresDbRow { values }
}

fn get_postgres_db_param(value: String) -> PostgresDbValue {
    if value.starts_with('[') && value.ends_with(']') {
        let mut elements: Vec<PostgresLazyDbValue> = vec![];
        let vs = value
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split(',');
        for v in vs {
            let element = PostgresLazyDbValue::new(PostgresDbValue::Text(v.trim().to_string()));
            elements.push(element)
        }
        PostgresDbValue::Array(elements)
    } else {
        PostgresDbValue::Text(value)
    }
}

fn do_sleep(sleep_ms: u64) {
    let c = 10;
    let s = std::time::Duration::from_millis(sleep_ms / c);
    for i in 0..c {
        println!("sleeping {i}/{c} for {s:?}");
        std::thread::sleep(s);
    }
}

#[agent_implementation]
impl RelationalDatabases for RelationalDatabasesImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn check(&self) -> String {
        let mysql_address = get_mysql_db_url().unwrap_or("N/A".to_string());
        let postgres_address = get_postgres_db_url().unwrap_or("N/A".to_string());
        println!(
            "check - postgres address: {}, mysql address: {}",
            postgres_address, mysql_address
        );
        "Ok".to_string()
    }

    fn postgres_operation_result(
        &self,
        operation: String,
        address: String,
        statement: String,
    ) -> Result<String, String> {
        let connection =
            PostgresDbConnection::open(&address).map_err(|error| format!("{error:?}"))?;
        match operation.as_str() {
            "open" => Ok("open"),
            "connection-query" => connection.query(&statement, vec![]).map(|_| "query"),
            "connection-query-stream" => connection
                .query_stream(&statement, vec![])
                .map(|_| "query-stream"),
            "connection-execute" => connection.execute(&statement, vec![]).map(|_| "execute"),
            "connection-begin-transaction" => {
                connection.begin_transaction().map(|_| "begin-transaction")
            }
            operation => {
                let transaction = connection
                    .begin_transaction()
                    .map_err(|error| format!("{error:?}"))?;
                match operation {
                    "transaction-query" => transaction.query(&statement, vec![]).map(|_| "query"),
                    "transaction-query-stream" => transaction
                        .query_stream(&statement, vec![])
                        .map(|_| "query-stream"),
                    "transaction-execute" => {
                        transaction.execute(&statement, vec![]).map(|_| "execute")
                    }
                    "transaction-commit" => transaction.commit().map(|_| "commit"),
                    "transaction-rollback" => transaction.rollback().map(|_| "rollback"),
                    _ => return Err(format!("unknown PostgreSQL operation: {operation}")),
                }
            }
        }
        .map(str::to_string)
        .map_err(|error| format!("{error:?}"))
    }

    fn mysql_operation_result(
        &self,
        operation: String,
        address: String,
        statement: String,
    ) -> Result<String, String> {
        let connection = MysqlDbConnection::open(&address).map_err(|error| format!("{error:?}"))?;
        match operation.as_str() {
            "open" => Ok("open".to_string()),
            "connection-query" => connection
                .query(&statement, &[])
                .map(|_| "query".to_string()),
            "connection-query-stream" => connection
                .query_stream(&statement, &[])
                .map(|_| "query-stream".to_string()),
            "connection-execute" => connection
                .execute(&statement, &[])
                .map(|_| "execute".to_string()),
            "connection-begin-transaction" => connection
                .begin_transaction()
                .map(|_| "begin-transaction".to_string()),
            operation => {
                let transaction = connection
                    .begin_transaction()
                    .map_err(|error| format!("{error:?}"))?;
                match operation {
                    "transaction-query" => transaction.query(&statement, &[]).map(|_| "query"),
                    "transaction-query-stream" => transaction
                        .query_stream(&statement, &[])
                        .map(|_| "query-stream"),
                    "transaction-execute" => {
                        transaction.execute(&statement, &[]).map(|_| "execute")
                    }
                    "transaction-commit" => transaction.commit().map(|_| "commit"),
                    "transaction-rollback" => transaction.rollback().map(|_| "rollback"),
                    _ => return Err(format!("unknown MySQL operation: {operation}")),
                }
                .map(str::to_string)
            }
        }
        .map_err(|error| format!("{error:?}"))
    }

    fn ignite_operation_result(
        &self,
        operation: String,
        address: String,
        statement: String,
    ) -> Result<String, String> {
        let connection =
            IgniteDbConnection::open(&address).map_err(|error| format!("{error:?}"))?;
        let params: &[IgniteDbValue] = &[];
        match operation.as_str() {
            "open" => Ok("open"),
            "connection-query" => connection.query(&statement, params).map(|_| "query"),
            "connection-query-stream" => connection
                .query_stream(&statement, params)
                .map(|_| "query-stream"),
            "connection-execute" => connection.execute(&statement, params).map(|_| "execute"),
            "connection-begin-transaction" => {
                connection.begin_transaction().map(|_| "begin-transaction")
            }
            operation => {
                let transaction = connection
                    .begin_transaction()
                    .map_err(|error| format!("{error:?}"))?;
                match operation {
                    "transaction-query" => transaction.query(&statement, params).map(|_| "query"),
                    "transaction-query-stream" => transaction
                        .query_stream(&statement, params)
                        .map(|_| "query-stream"),
                    "transaction-execute" => {
                        transaction.execute(&statement, params).map(|_| "execute")
                    }
                    "transaction-commit" => transaction.commit().map(|_| "commit"),
                    "transaction-rollback" => transaction.rollback().map(|_| "rollback"),
                    _ => return Err(format!("unknown Ignite operation: {operation}")),
                }
            }
        }
        .map(str::to_string)
        .map_err(|error| format!("{error:?}"))
    }

    fn postgres_executions(&self, statements: Vec<Statement>) -> Result<String, String> {
        let address = get_postgres_db_url()?;
        let connection = PostgresDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!(
            "postgres - address: {}, statements: {}",
            address,
            statements.len()
        );

        let mut results: Vec<String> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement
                .params
                .into_iter()
                .map(get_postgres_db_param)
                .collect::<Vec<PostgresDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let count = connection
                        .execute(&statement.statement, params)
                        .map_err(|e| e.to_string())?;
                    results.push(format!("execute:{count}"));
                }
                StatementAction::Query => {
                    let result = connection
                        .query(&statement.statement, params)
                        .map_err(|e| e.to_string())?;
                    let columns: Vec<_> = result
                        .columns
                        .into_iter()
                        .map(|c| {
                            let db_type = get_postgres_db_column_type(c.db_type);
                            format!("{}:{:?}", c.name, db_type)
                        })
                        .collect();
                    let rows: Vec<_> = result
                        .rows
                        .into_iter()
                        .map(|r| {
                            let row = get_postgres_db_row(r);
                            format!("{:?}", row.values)
                        })
                        .collect();
                    results.push(format!(
                        "query:columns=[{}],rows=[{}]",
                        columns.join(","),
                        rows.join(",")
                    ));
                }
                StatementAction::QueryStream => {
                    let result_set = connection
                        .query_stream(&statement.statement, params)
                        .map_err(|e| e.to_string())?;
                    let columns: Vec<_> = result_set
                        .get_columns()
                        .into_iter()
                        .map(|c| {
                            let db_type = get_postgres_db_column_type(c.db_type);
                            format!("{}:{:?}", c.name, db_type)
                        })
                        .collect();
                    let mut rows: Vec<String> = vec![];
                    while let Some(vs) = result_set.get_next() {
                        for r in vs {
                            let row = get_postgres_db_row(r);
                            rows.push(format!("{:?}", row.values));
                        }
                    }
                    results.push(format!(
                        "query_stream:columns=[{}],rows=[{}]",
                        columns.join(","),
                        rows.join(",")
                    ));
                }
            }

            if let Some(s) = statement.sleep {
                do_sleep(s);
            }
        }
        Ok(results.join(";"))
    }

    fn postgres_transaction(
        &self,
        statements: Vec<Statement>,
        end: TransactionEnd,
    ) -> Result<String, String> {
        let address = get_postgres_db_url()?;
        let connection = PostgresDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!(
            "postgres transaction - address: {}, statements: {}, end: {:?}",
            address,
            statements.len(),
            end
        );

        let transaction = connection.begin_transaction().map_err(|e| e.to_string())?;
        let mut results: Vec<String> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement
                .params
                .into_iter()
                .map(get_postgres_db_param)
                .collect::<Vec<PostgresDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let count = transaction
                        .execute(&statement.statement, params)
                        .map_err(|e| e.to_string())?;
                    results.push(format!("execute:{count}"));
                }
                StatementAction::Query => {
                    let result = transaction
                        .query(&statement.statement, params)
                        .map_err(|e| e.to_string())?;
                    let columns: Vec<_> = result
                        .columns
                        .into_iter()
                        .map(|c| {
                            let db_type = get_postgres_db_column_type(c.db_type);
                            format!("{}:{:?}", c.name, db_type)
                        })
                        .collect();
                    let rows: Vec<_> = result
                        .rows
                        .into_iter()
                        .map(|r| {
                            let row = get_postgres_db_row(r);
                            format!("{:?}", row.values)
                        })
                        .collect();
                    results.push(format!(
                        "query:columns=[{}],rows=[{}]",
                        columns.join(","),
                        rows.join(",")
                    ));
                }
                StatementAction::QueryStream => {
                    let result_set = transaction
                        .query_stream(&statement.statement, params)
                        .map_err(|e| e.to_string())?;
                    let columns: Vec<_> = result_set
                        .get_columns()
                        .into_iter()
                        .map(|c| {
                            let db_type = get_postgres_db_column_type(c.db_type);
                            format!("{}:{:?}", c.name, db_type)
                        })
                        .collect();
                    let mut rows: Vec<String> = vec![];
                    while let Some(vs) = result_set.get_next() {
                        for r in vs {
                            let row = get_postgres_db_row(r);
                            rows.push(format!("{:?}", row.values));
                        }
                    }
                    results.push(format!(
                        "query_stream:columns=[{}],rows=[{}]",
                        columns.join(","),
                        rows.join(",")
                    ));
                }
            }

            if let Some(s) = statement.sleep {
                do_sleep(s);
            }
        }

        match end {
            TransactionEnd::Commit => transaction.commit().map_err(|e| e.to_string())?,
            TransactionEnd::Rollback => transaction.rollback().map_err(|e| e.to_string())?,
            TransactionEnd::None => (),
        }

        Ok(results.join(";"))
    }

    fn mysql_executions(&self, statements: Vec<Statement>) -> Result<String, String> {
        let address = get_mysql_db_url().unwrap_or("N/A".to_string());
        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!(
            "mysql - address: {}, statements: {}",
            address,
            statements.len()
        );

        let mut results: Vec<String> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement
                .params
                .into_iter()
                .map(MysqlDbValue::Text)
                .collect::<Vec<MysqlDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let count = connection
                        .execute(&statement.statement, &params)
                        .map_err(|e| e.to_string())?;
                    results.push(format!("execute:{count}"));
                }
                StatementAction::Query => {
                    let result = connection
                        .query(&statement.statement, &params)
                        .map_err(|e| e.to_string())?;
                    results.push(format!(
                        "query:columns={},rows={}",
                        result.columns.len(),
                        result.rows.len()
                    ));
                }
                StatementAction::QueryStream => {
                    let result_set = connection
                        .query_stream(&statement.statement, &params)
                        .map_err(|e| e.to_string())?;
                    let columns = result_set.get_columns();
                    let mut row_count = 0;
                    while let Some(rs) = result_set.get_next() {
                        row_count += rs.len();
                    }
                    results.push(format!(
                        "query_stream:columns={},rows={}",
                        columns.len(),
                        row_count
                    ));
                }
            }

            if let Some(s) = statement.sleep {
                do_sleep(s);
            }
        }

        Ok(results.join(";"))
    }

    fn mysql_transaction(
        &self,
        statements: Vec<Statement>,
        end: TransactionEnd,
    ) -> Result<String, String> {
        let address = get_mysql_db_url().unwrap_or("N/A".to_string());
        let connection = MysqlDbConnection::open(&address).map_err(|e| e.to_string())?;

        println!(
            "mysql transaction - address: {}, statements: {}, end: {:?}",
            address,
            statements.len(),
            end
        );

        let transaction = connection.begin_transaction().map_err(|e| e.to_string())?;
        let mut results: Vec<String> = Vec::with_capacity(statements.len());

        for statement in statements.into_iter() {
            let params = statement
                .params
                .into_iter()
                .map(MysqlDbValue::Text)
                .collect::<Vec<MysqlDbValue>>();
            match statement.action {
                StatementAction::Execute => {
                    let count = transaction
                        .execute(&statement.statement, &params)
                        .map_err(|e| e.to_string())?;
                    results.push(format!("execute:{count}"));
                }
                StatementAction::Query => {
                    let result = transaction
                        .query(&statement.statement, &params)
                        .map_err(|e| e.to_string())?;
                    results.push(format!(
                        "query:columns={},rows={}",
                        result.columns.len(),
                        result.rows.len()
                    ));
                }
                StatementAction::QueryStream => {
                    let result_set = transaction
                        .query_stream(&statement.statement, &params)
                        .map_err(|e| e.to_string())?;
                    let columns = result_set.get_columns();
                    let mut row_count = 0;
                    while let Some(rs) = result_set.get_next() {
                        row_count += rs.len();
                    }
                    results.push(format!(
                        "query_stream:columns={},rows={}",
                        columns.len(),
                        row_count
                    ));
                }
            }

            if let Some(s) = statement.sleep {
                do_sleep(s);
            }
        }

        match end {
            TransactionEnd::Commit => transaction.commit().map_err(|e| e.to_string())?,
            TransactionEnd::Rollback => transaction.rollback().map_err(|e| e.to_string())?,
            TransactionEnd::None => (),
        }

        Ok(results.join(";"))
    }
}
