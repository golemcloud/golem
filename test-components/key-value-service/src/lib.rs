mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::keyvalue::eventual_batch::*;
use crate::bindings::wasi::keyvalue::eventual::{Bucket, OutgoingValue, delete, exists, get, set};

struct Component;

impl Guest for Component {
    fn delete(bucket: String, key: String) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        delete(&bucket, &key).unwrap()
    }

    fn delete_many(bucket: String, keys: Vec<String>) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        delete_many(&bucket, &keys).unwrap()
    }

    fn exists(bucket: String, key: String) -> bool {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        exists(&bucket, &key).unwrap()
    }

    fn get(bucket: String, key: String) -> Option<Vec<u8>> {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        match get(&bucket, &key) {
            Ok(Some(incoming_value)) => {
                let value = incoming_value.incoming_value_consume_sync().unwrap();
                Some(value)
            }
            Ok(None) => None,
            Err(error) => {
                let trace = error.trace();
                panic!("Unexpected error: {}", trace);
            }
        }
    }

    fn get_keys(bucket: String) -> Vec<String> {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        keys(&bucket).unwrap()
    }

    fn get_many(bucket: String, keys: Vec<String>) -> Option<Vec<Vec<u8>>> {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        match get_many(&bucket, &keys) {
            Ok(incoming_values) => {
                let maybe_values: Vec<_> = incoming_values
                    .into_iter()
                    .map(|incoming_value| {
                        incoming_value.map(|incoming_value| {
                            let value = incoming_value.incoming_value_consume_sync().unwrap();
                            value
                        })
                    })
                    .collect::<Vec<_>>()
                    .into();

                let mut result = Vec::new();
                for maybe_value in maybe_values {
                    match maybe_value {
                        Some(value) => result.push(value),
                        None => return None
                    }
                }
                Some(result)
            }
            Err(error) => {
                let trace = error.trace();
                panic!("Unexpected error: {}", trace);
            }
        }
    }

    fn set(bucket: String, key: String, value: Vec<u8>) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        let outgoing_value = OutgoingValue::new_outgoing_value();
        outgoing_value.outgoing_value_write_body_sync(&value).unwrap();
        set(&bucket, &key, &outgoing_value).unwrap()
    }

    fn set_many(bucket: String, key_values: Vec<(String, Vec<u8>)>) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        let mut outgoing_values = Vec::new();
        for (key, value) in key_values {
            let outgoing_value = OutgoingValue::new_outgoing_value();
            outgoing_value.outgoing_value_write_body_sync(&value).unwrap();
            outgoing_values.push((key, outgoing_value));
        }
        let outgoing_values_refs: Vec<_> = outgoing_values.iter().map(|(k, v)| (k.clone(), v)).collect();
        set_many(&bucket, outgoing_values_refs.as_slice()).unwrap()
    }
}

bindings::export!(Component with_types_in bindings);
