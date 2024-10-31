mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::rdbms::postgres::DbConnection;
use crate::bindings::wasi::rdbms::types::{DbValue, DbValuePrimitive, DbRow};


fn get_db_url() -> String {
    std::env::var("DB_URL").expect("DB_URL not set")
}

struct Component;

impl Guest for Component {

    fn execute(statement: String, params: Vec<String>) -> Result<u64, String> {

        let address = get_db_url();

        let connection  = DbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("execute - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.iter().map(|v| DbValue::Primitive(DbValuePrimitive::Chars(v.clone()))).collect::<Vec<DbValue>>();

        let result = connection.execute(&statement, &params).map_err(|e| e.to_string());

        println!("execute result: {:?}", result);

        result
    }

    fn query(statement: String, params: Vec<String>) -> Result<Vec<DbRow>, String> {

        let address = get_db_url();

        let connection  = DbConnection::open(&address).map_err(|e| e.to_string())?;

        println!("query - address: {}, statement: {}, params: {:?}", address, statement, params);

        let params = params.iter().map(|v| DbValue::Primitive(DbValuePrimitive::Chars(v.clone()))).collect::<Vec<DbValue>>();

        let result_set = connection.query(&statement, &params).map_err(|e| e.to_string())?;

        let mut rows: Vec<DbRow> = vec![];
        loop {
            match result_set.get_next() {
                Some(values) => {
                    rows.extend(values);
                }
                None => break,
            }
        }

        println!("query result: {:?}", rows);

        Ok(rows)
    }


}

bindings::export!(Component with_types_in bindings);
