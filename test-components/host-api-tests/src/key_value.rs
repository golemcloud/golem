use golem_rust::bindings::wasi::keyvalue::eventual::{
    Bucket, OutgoingValue, delete, exists, get, set,
};
use golem_rust::bindings::wasi::keyvalue::eventual_batch::{delete_many, get_many, keys, set_many};
use golem_rust::{agent_definition, agent_implementation, wit_stream};

#[agent_definition]
pub trait KeyValue {
    fn new(name: String) -> Self;
    fn delete(&self, bucket: String, key: String);
    fn delete_many(&self, bucket: String, keys: Vec<String>);
    fn exists(&self, bucket: String, key: String) -> bool;
    fn get(&self, bucket: String, key: String) -> Option<Vec<u8>>;
    fn get_keys(&self, bucket: String) -> Vec<String>;
    fn get_many(&self, bucket: String, keys: Vec<String>) -> Option<Vec<Vec<u8>>>;
    fn set(&self, bucket: String, key: String, value: Vec<u8>);
    async fn set_using_async_body(&self, bucket: String, key: String, value: Vec<u8>);
    fn set_many(&self, bucket: String, key_values: Vec<(String, Vec<u8>)>);
}

pub struct KeyValueImpl {
    _name: String,
}

#[agent_implementation]
impl KeyValue for KeyValueImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn delete(&self, bucket: String, key: String) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        delete(&bucket, &key).unwrap()
    }

    fn delete_many(&self, bucket: String, keys_list: Vec<String>) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        delete_many(&bucket, &keys_list).unwrap()
    }

    fn exists(&self, bucket: String, key: String) -> bool {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        exists(&bucket, &key).unwrap()
    }

    fn get(&self, bucket: String, key: String) -> Option<Vec<u8>> {
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

    fn get_keys(&self, bucket: String) -> Vec<String> {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        keys(&bucket).unwrap()
    }

    fn get_many(&self, bucket: String, keys_list: Vec<String>) -> Option<Vec<Vec<u8>>> {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        match get_many(&bucket, &keys_list) {
            Ok(incoming_values) => {
                let maybe_values: Vec<_> = incoming_values
                    .into_iter()
                    .map(|incoming_value| {
                        incoming_value.map(|incoming_value| {
                            incoming_value.incoming_value_consume_sync().unwrap()
                        })
                    })
                    .collect();

                let mut result = Vec::new();
                for maybe_value in maybe_values {
                    result.push(maybe_value?);
                }
                Some(result)
            }
            Err(error) => {
                let trace = error.trace();
                panic!("Unexpected error: {}", trace);
            }
        }
    }

    fn set(&self, bucket: String, key: String, value: Vec<u8>) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        let outgoing_value = OutgoingValue::new_outgoing_value();
        outgoing_value
            .outgoing_value_write_body_sync(&value)
            .unwrap();
        set(&bucket, &key, &outgoing_value).unwrap()
    }

    /// Writes the value into the outgoing value's body in multiple chunks using
    /// the WASI P3 stream-based `outgoing-value-write-body-async` function: the
    /// guest creates a `stream<u8>`, hands the readable end to the host, then
    /// writes the bytes into the writable end.
    async fn set_using_async_body(&self, bucket: String, key: String, value: Vec<u8>) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        let outgoing_value = OutgoingValue::new_outgoing_value();
        let (mut writer, reader) = wit_stream::new::<u8>();
        outgoing_value
            .outgoing_value_write_body_async(reader)
            .unwrap();
        let mid = value.len() / 2;
        let (first, second) = value.split_at(mid);
        let remaining = writer.write_all(first.to_vec()).await;
        assert!(
            remaining.is_empty(),
            "host did not consume the first body chunk"
        );
        let remaining = writer.write_all(second.to_vec()).await;
        assert!(
            remaining.is_empty(),
            "host did not consume the second body chunk"
        );
        drop(writer);
        set(&bucket, &key, &outgoing_value).unwrap()
    }

    fn set_many(&self, bucket: String, key_values: Vec<(String, Vec<u8>)>) {
        let bucket = Bucket::open_bucket(&bucket).unwrap();
        let mut outgoing_values = Vec::new();
        for (key, value) in key_values {
            let outgoing_value = OutgoingValue::new_outgoing_value();
            outgoing_value
                .outgoing_value_write_body_sync(&value)
                .unwrap();
            outgoing_values.push((key, outgoing_value));
        }
        let outgoing_values_refs: Vec<_> = outgoing_values
            .iter()
            .map(|(k, v)| (k.clone(), v))
            .collect();
        set_many(&bucket, outgoing_values_refs.as_slice()).unwrap()
    }
}
