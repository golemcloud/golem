use golem_rust::bindings::golem::rdbms::postgres::DbConnection;
use golem_rust::bindings::wasi::keyvalue::eventual::{Bucket, OutgoingValue, set};
use golem_rust::{agent_definition, agent_implementation};

fn set_twice(bucket: String) {
    let bucket = Bucket::open_bucket(&bucket).unwrap();
    let value = OutgoingValue::new_outgoing_value();
    value.outgoing_value_write_body_sync(&[19]).unwrap();
    set(&bucket, "first", &value).unwrap();
    set(&bucket, "second", &value).unwrap();
}

fn execute_twice(address: String) {
    let connection = DbConnection::open(&address).unwrap();
    assert_eq!(
        connection
            .execute(
                "INSERT INTO completion_effects (label) VALUES ('first')",
                vec![]
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .execute(
                "INSERT INTO completion_effects (label) VALUES ('second')",
                vec![]
            )
            .unwrap(),
        1
    );
}

fn commit(address: String) {
    let connection = DbConnection::open(&address).unwrap();
    let transaction = connection.begin_transaction().unwrap();
    assert_eq!(
        transaction
            .execute(
                "INSERT INTO completion_effects (label) VALUES ('tx-statement')",
                vec![],
            )
            .unwrap(),
        1
    );
    transaction.commit().unwrap();
}

#[agent_definition]
pub trait DbKvCompletion {
    fn new(name: String) -> Self;
    fn set_twice(&self, bucket: String);
    fn execute_twice(&self, address: String);
    fn commit(&self, address: String);
}

pub struct DbKvCompletionImpl;

#[agent_implementation]
impl DbKvCompletion for DbKvCompletionImpl {
    fn new(_name: String) -> Self {
        Self
    }
    fn set_twice(&self, bucket: String) {
        set_twice(bucket);
    }
    fn execute_twice(&self, address: String) {
        execute_twice(address);
    }
    fn commit(&self, address: String) {
        commit(address);
    }
}

#[agent_definition(mode = "ephemeral")]
pub trait EphemeralDbKvCompletion {
    fn new(name: String) -> Self;
    fn set_twice(&self, bucket: String);
    fn execute_twice(&self, address: String);
}

pub struct EphemeralDbKvCompletionImpl;

#[agent_implementation]
impl EphemeralDbKvCompletion for EphemeralDbKvCompletionImpl {
    fn new(_name: String) -> Self {
        Self
    }
    fn set_twice(&self, bucket: String) {
        set_twice(bucket);
    }
    fn execute_twice(&self, address: String) {
        execute_twice(address);
    }
}
