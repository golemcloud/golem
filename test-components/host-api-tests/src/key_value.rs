use golem_rust::bindings::wasi::keyvalue::cache;
use golem_rust::bindings::wasi::keyvalue::eventual::{
    Bucket, OutgoingValue, delete, exists, get, set,
};
use golem_rust::bindings::wasi::keyvalue::eventual_batch::{delete_many, get_many, keys, set_many};
use golem_rust::{
    PromiseId, agent_definition, agent_implementation, await_promise, create_promise, wit_stream,
};

#[agent_definition]
pub trait KeyValue {
    fn new(name: String) -> Self;
    fn create_release_promise(&self) -> PromiseId;
    fn delete(&self, bucket: String, key: String);
    fn delete_many(&self, bucket: String, keys: Vec<String>);
    fn exists(&self, bucket: String, key: String) -> bool;
    fn get(&self, bucket: String, key: String) -> Option<Vec<u8>>;
    fn get_keys(&self, bucket: String) -> Vec<String>;
    fn get_many(&self, bucket: String, keys: Vec<String>) -> Option<Vec<Vec<u8>>>;
    fn set(&self, bucket: String, key: String, value: Vec<u8>);
    async fn set_using_async_body(&self, bucket: String, key: String, value: Vec<u8>);
    fn set_many(&self, bucket: String, key_values: Vec<(String, Vec<u8>)>);
    fn get_result(&self, bucket: String, key: String) -> Result<Option<Vec<u8>>, String>;
    fn get_many_result(
        &self,
        bucket: String,
        keys: Vec<String>,
    ) -> Result<Vec<Option<Vec<u8>>>, String>;
    fn set_result(&self, bucket: String, key: String, value: Vec<u8>) -> Result<(), String>;
    fn set_many_result(
        &self,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), String>;
    async fn cache_set_result(&self, key: String, value: Vec<u8>) -> Result<(), String>;
    async fn cache_fill_after_promise(
        &self,
        key: String,
        value: Vec<u8>,
        release: PromiseId,
    ) -> Result<(), String>;
    fn eventual_probe(
        &self,
        operation: String,
        bucket: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), String>;
    fn eventual_batch_probe(
        &self,
        operation: String,
        bucket: String,
        keys: Vec<String>,
        value: Vec<u8>,
    ) -> Result<(), String>;
    async fn cache_probe(
        &self,
        operation: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), String>;
}

pub struct KeyValueImpl {
    _name: String,
}

#[agent_implementation]
impl KeyValue for KeyValueImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn create_release_promise(&self) -> PromiseId {
        create_promise()
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

    fn get_result(&self, bucket: String, key: String) -> Result<Option<Vec<u8>>, String> {
        let bucket = Bucket::open_bucket(&bucket).map_err(|error| format!("{error:?}"))?;
        get(&bucket, &key)
            .map_err(|error| format!("{error:?}"))?
            .map(|value| {
                value
                    .incoming_value_consume_sync()
                    .map_err(|error| format!("{error:?}"))
            })
            .transpose()
    }

    fn get_many_result(
        &self,
        bucket: String,
        keys_list: Vec<String>,
    ) -> Result<Vec<Option<Vec<u8>>>, String> {
        let bucket = Bucket::open_bucket(&bucket).map_err(|error| format!("{error:?}"))?;
        get_many(&bucket, &keys_list)
            .map_err(|error| format!("{error:?}"))?
            .into_iter()
            .map(|value| {
                value
                    .map(|value| {
                        value
                            .incoming_value_consume_sync()
                            .map_err(|error| format!("{error:?}"))
                    })
                    .transpose()
            })
            .collect()
    }

    fn set_result(&self, bucket: String, key: String, value: Vec<u8>) -> Result<(), String> {
        let bucket = Bucket::open_bucket(&bucket).map_err(|error| format!("{error:?}"))?;
        let outgoing_value = OutgoingValue::new_outgoing_value();
        outgoing_value
            .outgoing_value_write_body_sync(&value)
            .map_err(|error| format!("{error:?}"))?;
        set(&bucket, &key, &outgoing_value).map_err(|error| format!("{error:?}"))
    }

    fn set_many_result(
        &self,
        bucket: String,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), String> {
        let bucket = Bucket::open_bucket(&bucket).map_err(|error| format!("{error:?}"))?;
        let mut outgoing_values = Vec::new();
        for (key, value) in key_values {
            let outgoing_value = OutgoingValue::new_outgoing_value();
            outgoing_value
                .outgoing_value_write_body_sync(&value)
                .map_err(|error| format!("{error:?}"))?;
            outgoing_values.push((key, outgoing_value));
        }
        let refs = outgoing_values
            .iter()
            .map(|(key, value)| (key.clone(), value))
            .collect::<Vec<_>>();
        set_many(&bucket, &refs).map_err(|error| format!("{error:?}"))
    }

    async fn cache_set_result(&self, key: String, value: Vec<u8>) -> Result<(), String> {
        let outgoing_value = OutgoingValue::new_outgoing_value();
        outgoing_value
            .outgoing_value_write_body_sync(&value)
            .map_err(|error| format!("{error:?}"))?;
        cache::set(&key, &outgoing_value, None)
            .get()
            .await
            .map_err(|error| format!("{}", error.trace()))
    }

    async fn cache_fill_after_promise(
        &self,
        key: String,
        value: Vec<u8>,
        release: PromiseId,
    ) -> Result<(), String> {
        let vacancy = match cache::get_or_set(&key)
            .get()
            .await
            .map_err(|error| format!("{}", error.trace()))?
        {
            cache::GetOrSetEntry::Vacant(vacancy) => vacancy,
            cache::GetOrSetEntry::Occupied(_) => {
                return Err("cache entry is occupied".to_string());
            }
        };
        await_promise(&release).await;
        let outgoing_value = vacancy.vacancy_fill(None);
        outgoing_value
            .outgoing_value_write_body_sync(&value)
            .map_err(|error| format!("{error:?}"))
    }

    fn eventual_probe(
        &self,
        operation: String,
        bucket: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), String> {
        let bucket = Bucket::open_bucket(&bucket).map_err(|error| format!("{error:?}"))?;
        match operation.as_str() {
            "get" => get(&bucket, &key)
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
            "exists" => exists(&bucket, &key)
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
            "set" => {
                let outgoing_value = OutgoingValue::new_outgoing_value();
                outgoing_value
                    .outgoing_value_write_body_sync(&value)
                    .map_err(|error| format!("{error:?}"))?;
                set(&bucket, &key, &outgoing_value).map_err(|error| format!("{error:?}"))
            }
            "delete" => delete(&bucket, &key).map_err(|error| format!("{error:?}")),
            _ => Err(format!("unknown eventual operation: {operation}")),
        }
    }

    fn eventual_batch_probe(
        &self,
        operation: String,
        bucket: String,
        keys_list: Vec<String>,
        value: Vec<u8>,
    ) -> Result<(), String> {
        let bucket = Bucket::open_bucket(&bucket).map_err(|error| format!("{error:?}"))?;
        match operation.as_str() {
            "get-many" => get_many(&bucket, &keys_list)
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
            "keys" => keys(&bucket)
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
            "set-many" => {
                let mut outgoing_values = Vec::new();
                for key in keys_list {
                    let outgoing_value = OutgoingValue::new_outgoing_value();
                    outgoing_value
                        .outgoing_value_write_body_sync(&value)
                        .map_err(|error| format!("{error:?}"))?;
                    outgoing_values.push((key, outgoing_value));
                }
                let refs = outgoing_values
                    .iter()
                    .map(|(key, value)| (key.clone(), value))
                    .collect::<Vec<_>>();
                set_many(&bucket, &refs).map_err(|error| format!("{error:?}"))
            }
            "delete-many" => delete_many(&bucket, &keys_list).map_err(|error| format!("{error:?}")),
            _ => Err(format!("unknown eventual-batch operation: {operation}")),
        }
    }

    async fn cache_probe(
        &self,
        operation: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), String> {
        match operation.as_str() {
            "get" => cache::get(&key).get().await.map(|_| ()),
            "exists" => cache::exists(&key).get().await.map(|_| ()),
            "set" => {
                let outgoing_value = OutgoingValue::new_outgoing_value();
                outgoing_value
                    .outgoing_value_write_body_sync(&value)
                    .map_err(|error| format!("{error:?}"))?;
                cache::set(&key, &outgoing_value, None).get().await
            }
            "get-or-set" => cache::get_or_set(&key).get().await.map(|_| ()),
            "delete" => cache::delete(&key).get().await,
            "vacancy-fill" => {
                return match cache::get_or_set(&key)
                    .get()
                    .await
                    .map_err(|error| format!("{}", error.trace()))?
                {
                    cache::GetOrSetEntry::Vacant(vacancy) => {
                        vacancy.vacancy_fill(None);
                        Ok(())
                    }
                    cache::GetOrSetEntry::Occupied(_) => Err("cache entry is occupied".to_string()),
                };
            }
            _ => return Err(format!("unknown cache operation: {operation}")),
        }
        .map_err(|error| format!("{}", error.trace()))
    }
}
