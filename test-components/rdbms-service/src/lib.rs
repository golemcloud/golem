mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::rdbms::postgres::DbConnection;
use crate::bindings::wasi::rdbms::types::{DbValue, DbValuePrimitive};


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


}

bindings::export!(Component with_types_in bindings);
